# Custom Guards

Guards run authorization checks before the handler body. R2E supports two guard types: post-auth (`Guard`) and pre-auth (`PreAuthGuard`).

## Post-auth guards

Post-auth guards run after JWT validation and have access to the identity:

```rust
use r2e_core::{Guard, GuardContext, Identity, AppError};
use axum::response::{IntoResponse, Response};

struct TenantGuard;

impl<S: Send + Sync, I: Identity> Guard<S, I> for TenantGuard {
    fn check(
        &self,
        _state: &S,
        ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async move {
            let tenant_id = ctx.uri().path().split('/').nth(2);
            let user_tenant = ctx.identity_claims()
                .and_then(|c| c["tenant_id"].as_str());

            match (tenant_id, user_tenant) {
                (Some(path_tenant), Some(jwt_tenant)) if path_tenant == jwt_tenant => Ok(()),
                _ => Err(AppError::Forbidden("Tenant mismatch".into()).into_response()),
            }
        }
    }
}
```

Apply with `#[guard(...)]`:

```rust
#[get("/{tenant_id}/data")]
#[guard(TenantGuard)]
async fn get_tenant_data(&self) -> Json<Data> { /* ... */ }
```

## Pre-auth guards

Pre-auth guards run before JWT validation — useful for checks that don't need identity:

```rust
use r2e_core::{PreAuthGuard, PreAuthGuardContext};

struct MaintenanceGuard;

impl<S: Send + Sync> PreAuthGuard<S> for MaintenanceGuard {
    fn check(
        &self,
        _state: &S,
        _ctx: &PreAuthGuardContext<'_>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async move {
            if is_maintenance_mode() {
                Err(AppError::Custom {
                    status: StatusCode::SERVICE_UNAVAILABLE,
                    body: serde_json::json!({"error": "Under maintenance"}),
                }.into_response())
            } else {
                Ok(())
            }
        }
    }
}

#[get("/")]
#[pre_guard(MaintenanceGuard)]
async fn list(&self) -> Json<Vec<Item>> { /* ... */ }
```

## Guards with state access

Guards can access the application state for database lookups or configuration:

```rust
struct ActiveUserGuard;

impl<S: Send + Sync, I: Identity> Guard<S, I> for ActiveUserGuard
where
    SqlitePool: FromRef<S>,
{
    fn check(
        &self,
        state: &S,
        ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async move {
            let pool = SqlitePool::from_ref(state);
            let sub = ctx.identity_sub().unwrap_or("");

            let active = sqlx::query_scalar::<_, bool>(
                "SELECT active FROM users WHERE sub = ?"
            )
            .bind(sub)
            .fetch_optional(&pool)
            .await
            .map_err(|_| AppError::Internal("DB error".into()).into_response())?;

            match active {
                Some(true) => Ok(()),
                _ => Err(AppError::Forbidden("Account suspended".into()).into_response()),
            }
        }
    }
}
```

## Guard context

### Post-auth `GuardContext<I>`

| Field/Method | Type | Description |
|--------------|------|-------------|
| `method_name` | `&str` | Handler method name |
| `controller_name` | `&str` | Controller struct name |
| `headers` | `&HeaderMap` | Request headers |
| `uri` | `&Uri` | Request URI |
| `identity` | `Option<&I>` | Authenticated identity |
| `identity_sub()` | `Option<&str>` | Subject from identity |
| `identity_roles()` | `Option<&[String]>` | Roles from identity |
| `identity_email()` | `Option<&str>` | Email from identity |
| `identity_claims()` | `Option<&Value>` | Raw JWT claims |
| `path()` | `&str` | Request path |
| `query_string()` | `Option<&str>` | Query string |

### Pre-auth `PreAuthGuardContext`

Same as above but without `identity` (and no identity-related methods).

## Combining guards

```rust
#[post("/")]
#[pre_guard(MaintenanceGuard)]                // pre-auth checks
#[pre_guard(RateLimit::per_ip(10, 60))]       // IP rate limit
#[roles("editor")]                             // role check
#[guard(TenantGuard)]                          // custom post-auth
#[guard(ActiveUserGuard)]                      // another post-auth
async fn create(&self, body: Json<Request>) -> Json<Response> {
    // Reached only if ALL guards pass
}
```

Guards execute in order and short-circuit on first failure.
