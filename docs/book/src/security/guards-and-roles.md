# Guards and Roles

Guards are authorization checks that run before the handler body. R2E supports role-based access control and custom guards.

## Role-based access

Use `#[roles("...")]` to restrict endpoint access:

```rust
#[get("/admin")]
#[roles("admin")]
async fn admin_only(&self) -> Json<&'static str> {
    Json("secret admin data")
}

// Multiple roles (OR logic — user needs at least one)
#[get("/manage")]
#[roles("admin", "moderator")]
async fn manage(&self) -> Json<&'static str> {
    Json("management panel")
}
```

If the user doesn't have the required role, a 403 Forbidden response is returned.

## The `Guard` trait

Custom post-auth guards implement `Guard<S, I>`:

```rust
use r2e_core::{Guard, GuardContext, Identity};
use axum::response::{IntoResponse, Response};

struct TenantGuard;

impl<S: Send + Sync, I: Identity> Guard<S, I> for TenantGuard {
    fn check(
        &self,
        state: &S,
        ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async move {
            match ctx.identity_claims() {
                Some(claims) if claims["tenant_id"].is_string() => Ok(()),
                _ => Err(AppError::Forbidden("Missing tenant".into()).into_response()),
            }
        }
    }
}
```

Apply with `#[guard(...)]`:

```rust
#[get("/")]
#[guard(TenantGuard)]
async fn tenant_data(&self) -> Json<Data> { /* ... */ }
```

### `GuardContext`

Guards receive a `GuardContext` with:

| Field | Type | Description |
|-------|------|-------------|
| `method_name` | `&str` | Handler method name |
| `controller_name` | `&str` | Controller struct name |
| `headers` | `&HeaderMap` | Request headers |
| `uri` | `&Uri` | Request URI |
| `identity` | `Option<&I>` | Authenticated identity (if available) |

Convenience methods: `identity_sub()`, `identity_roles()`, `identity_email()`, `identity_claims()`, `path()`, `query_string()`.

### The `Identity` trait

Guards are generic over the `Identity` trait, decoupling them from the concrete `AuthenticatedUser` type:

```rust
pub trait Identity: Send + Sync {
    fn sub(&self) -> &str;
    fn roles(&self) -> &[String];
    fn email(&self) -> Option<&str> { None }
    fn claims(&self) -> Option<&serde_json::Value> { None }
}
```

`AuthenticatedUser` implements `Identity`. You can create custom identity types by implementing this trait.

## Pre-auth guards

For authorization that doesn't need identity (e.g., IP allowlisting), use `PreAuthGuard`:

```rust
use r2e_core::{PreAuthGuard, PreAuthGuardContext};

struct IpAllowlistGuard;

impl<S: Send + Sync> PreAuthGuard<S> for IpAllowlistGuard {
    fn check(
        &self,
        state: &S,
        ctx: &PreAuthGuardContext<'_>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async move {
            let ip = ctx.headers.get("x-forwarded-for")
                .and_then(|v| v.to_str().ok());
            match ip {
                Some("10.0.0.1") => Ok(()),
                _ => Err(AppError::Forbidden("IP not allowed".into()).into_response()),
            }
        }
    }
}

#[get("/")]
#[pre_guard(IpAllowlistGuard)]
async fn restricted(&self) -> &'static str { "allowed" }
```

Pre-auth guards run **before** JWT extraction, avoiding wasted token validation.

## Async guards with database access

Guards can perform async operations like database lookups:

```rust
struct DatabaseGuard;

impl<S: Send + Sync, I: Identity> Guard<S, I> for DatabaseGuard
where
    sqlx::SqlitePool: FromRef<S>,
{
    fn check(
        &self,
        state: &S,
        ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async move {
            let pool = sqlx::SqlitePool::from_ref(state);
            let allowed = sqlx::query_scalar::<_, bool>(
                "SELECT active FROM users WHERE sub = ?"
            )
            .bind(ctx.identity_sub().unwrap_or(""))
            .fetch_optional(&pool)
            .await
            .map_err(|_| AppError::Internal("DB error".into()).into_response())?;

            match allowed {
                Some(true) => Ok(()),
                _ => Err(AppError::Forbidden("Account suspended".into()).into_response()),
            }
        }
    }
}
```

## Guard execution order

1. Pre-auth guards (`#[pre_guard(...)]`) — run before JWT validation
2. Rate limit guards (`RateLimit::per_user(...)`) — run after JWT validation
3. Role guards (`#[roles("...")]`) — check identity roles
4. Custom guards (`#[guard(...)]`) — run after roles

Guards short-circuit on first failure — later guards don't run.

## Combining guards

```rust
#[post("/")]
#[pre_guard(RateLimit::per_ip(10, 60))]      // Pre-auth: IP rate limit
#[guard(RateLimit::per_user(5, 60))]          // Post-auth: user rate limit
#[roles("editor")]                             // Role check
#[guard(TenantGuard)]                          // Custom check
async fn create(&self, body: Json<Request>) -> Json<Response> {
    // Only reached if ALL guards pass
}
```
