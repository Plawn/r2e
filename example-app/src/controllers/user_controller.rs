use crate::models::{CreateUserRequest, User};
use crate::services::UserService;
use crate::state::Services;
use axum::extract::Path;
use quarlus_security::AuthenticatedUser;
use tracing::info;

quarlus_macros::controller! {
    impl UserController for Services {
        #[inject]
        user_service: UserService,

        #[inject]
        pool: sqlx::SqlitePool,

        #[identity]
        user: AuthenticatedUser,

        #[get("/users")]
        async fn list(&self) -> axum::Json<Vec<User>> {
            let users = self.user_service.list().await;
            axum::Json(users)
        }

        #[get("/users/{id}")]
        async fn get_by_id(
            &self,
            Path(id): Path<u64>,
        ) -> Result<axum::Json<User>, quarlus_core::AppError> {
            match self.user_service.get_by_id(id).await {
                Some(user) => Ok(axum::Json(user)),
                None => Err(quarlus_core::AppError::NotFound("User not found".into())),
            }
        }

        #[post("/users")]
        async fn create(
            &self,
            axum::Json(body): axum::Json<CreateUserRequest>,
        ) -> axum::Json<User> {
            let user = self.user_service.create(body.name, body.email).await;
            axum::Json(user)
        }

        #[post("/users/db")]
        #[transactional]
        async fn create_in_db(
            &self,
            axum::Json(body): axum::Json<CreateUserRequest>,
        ) -> Result<axum::Json<User>, quarlus_core::AppError> {
            sqlx::query("INSERT INTO users (name, email) VALUES (?, ?)")
                .bind(&body.name)
                .bind(&body.email)
                .execute(&mut *tx)
                .await
                .map_err(|e| quarlus_core::AppError::Internal(e.to_string()))?;

            let row = sqlx::query_as::<_, (i64, String, String)>(
                "SELECT id, name, email FROM users WHERE rowid = last_insert_rowid()",
            )
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| quarlus_core::AppError::Internal(e.to_string()))?;

            let r = Ok(axum::Json(User {
                id: row.0 as u64,
                name: row.1,
                email: row.2,
            }));
            tracing::info!("users are {:?}", &r);
            r
        }

        #[get("/me")]
        async fn me(&self) -> axum::Json<AuthenticatedUser> {
            axum::Json(self.user.clone())
        }

        #[get("/admin/users")]
        #[roles("admin")]
        async fn admin_list(&self) -> axum::Json<Vec<User>> {
            let users = self.user_service.list().await;
            axum::Json(users)
        }
    }
}
