use crate::models::{CreateUserRequest, User};
use crate::services::UserService;
use crate::state::Services;
use r2e::prelude::*;
use r2e::r2e_rate_limit::RateLimit;
use sqlx::Sqlite;
use std::future::Future;

/// A custom user-defined interceptor for audit logging (generic — no state access).
pub struct AuditLog;

impl<R: Send, S: Send + Sync> Interceptor<R, S> for AuditLog {
    fn around<F, Fut>(
        &self,
        ctx: InterceptorContext<'_, S>,
        next: F,
    ) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        let method_name = ctx.method_name;
        let controller_name = ctx.controller_name;
        async move {
            tracing::info!(
                method = method_name,
                controller = controller_name,
                "audit: entering"
            );
            let result = next().await;
            tracing::info!(
                method = method_name,
                controller = controller_name,
                "audit: done"
            );
            result
        }
    }
}

/// A stateful interceptor that accesses the application state.
///
/// Unlike `AuditLog` (generic over `S`), this interceptor is bound to `Services`
/// and can access the database pool, event bus, or any other field from the state.
///
/// # Usage
/// ```ignore
/// #[intercept(DbAuditLog)]
/// async fn create(&self, body: Json<User>) -> Result<Json<User>, AppError> { ... }
/// ```
pub struct DbAuditLog;

impl<R: Send> Interceptor<R, Services> for DbAuditLog {
    fn around<F, Fut>(
        &self,
        ctx: InterceptorContext<'_, Services>,
        next: F,
    ) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        let method_name = ctx.method_name;
        let pool = ctx.state.pool.clone();
        async move {
            let result = next().await;
            // Write an audit log entry to the database after execution
            let _ = sqlx::query("INSERT INTO audit_log (method, ts) VALUES (?, datetime('now'))")
                .bind(method_name)
                .execute(&pool)
                .await;
            result
        }
    }
}

#[derive(Controller)]
#[controller(path = "/users", state = Services)]
pub struct UserController {
    #[inject]
    user_service: UserService,

    #[inject]
    pool: sqlx::SqlitePool,

    #[inject(identity)]
    user: AuthenticatedUser,

    #[config("app.greeting")]
    greeting: String,
}

#[routes]
#[intercept(Logged::info())]
impl UserController {
    // Demo: logged with custom level + timed with threshold
    #[get("/")]
    #[intercept(Logged::debug())]
    #[intercept(Timed::threshold(50))]
    async fn list(&self) -> Json<Vec<User>> {
        let users = self.user_service.list().await;
        Json(users)
    }

    #[get("/{id}")]
    async fn get_by_id(
        &self,
        Path(id): Path<u64>,
    ) -> Result<Json<User>, AppError> {
        match self.user_service.get_by_id(id).await {
            Some(user) => Ok(Json(user)),
            None => Err(AppError::NotFound("User not found".into())),
        }
    }

    // Demo: cache_invalidate clears the "users" cache group on create
    #[post("/")]
    #[intercept(CacheInvalidate::group("users"))]
    async fn create(
        &self,
        Validated(body): Validated<CreateUserRequest>,
    ) -> Json<User> {
        let user = self.user_service.create(body.name, body.email).await;
        Json(user)
    }

    #[post("/db")]
    async fn create_in_db(
        &self,
        Json(body): Json<CreateUserRequest>,
        #[managed] tx: &mut Tx<'_, Sqlite>,
    ) -> Result<Json<User>, AppError> {
        sqlx::query("INSERT INTO users (name, email) VALUES (?, ?)")
            .bind(&body.name)
            .bind(&body.email)
            .execute(tx.as_mut())
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;

        let row = sqlx::query_as::<_, (i64, String, String)>(
            "SELECT id, name, email FROM users WHERE rowid = last_insert_rowid()",
        )
        .fetch_one(tx.as_mut())
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

        Ok(Json(User {
            id: row.0 as u64,
            name: row.1,
            email: row.2,
        }))
    }

    // Demo: cached with group name (shared with cache_invalidate on create)
    #[get("/cached")]
    #[intercept(Cache::ttl(30).group("users"))]
    #[intercept(Timed::info())]
    async fn cached_list(&self) -> Json<serde_json::Value> {
        let users = self.user_service.list().await;
        Json(serde_json::to_value(users).unwrap())
    }

    // Demo: rate limiting at handler level with per-user key
    #[post("/rate-limited")]
    #[guard(RateLimit::per_user(5, 60))]
    async fn create_rate_limited(
        &self,
        Validated(body): Validated<CreateUserRequest>,
    ) -> Json<User> {
        let user = self.user_service.create(body.name, body.email).await;
        Json(user)
    }

    // Demo: custom interceptor via #[intercept] (generic, no state access)
    #[get("/audited")]
    #[intercept(Logged::info())]
    #[intercept(AuditLog)]
    async fn audited_list(&self) -> Json<Vec<User>> {
        let users = self.user_service.list().await;
        Json(users)
    }

    // Demo: stateful interceptor that accesses ctx.state (writes audit log to DB)
    #[get("/db-audited")]
    #[intercept(DbAuditLog)]
    async fn db_audited_list(&self) -> Json<Vec<User>> {
        let users = self.user_service.list().await;
        Json(users)
    }
}
