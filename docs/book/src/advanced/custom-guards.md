# Custom Guards

Guards run authorization checks before the handler body. R2E supports two guard types: post-auth (`Guard`) and pre-auth (`PreAuthGuard`).

## Post-auth guards

Post-auth guards run after JWT validation and have access to the identity.
Guards no longer receive the application state — a guard that reads no beans is
a **self-contained decorator**: implement `Guard<I>` and opt in with one line,
`impl SelfBuilt for MyGuard {}`.

```rust
use r2e::prelude::*; // Guard, GuardContext, Identity, SelfBuilt, HttpError, IntoResponse, Response

struct TenantGuard;

// No bean dependencies → self-contained: the attribute expression is already
// the finished guard.
impl SelfBuilt for TenantGuard {}

impl<I: Identity> Guard<I> for TenantGuard {
    fn check(
        &self,
        ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async move {
            let tenant_id = ctx.path().split('/').nth(2);
            let user_tenant = ctx.identity_claims()
                .and_then(|c| c["tenant_id"].as_str());

            match (tenant_id, user_tenant) {
                (Some(path_tenant), Some(jwt_tenant)) if path_tenant == jwt_tenant => Ok(()),
                _ => Err(HttpError::Forbidden("Tenant mismatch".into()).into_response()),
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
use r2e::prelude::*; // PreAuthGuard, PreAuthGuardContext, SelfBuilt, HttpError, StatusCode

struct MaintenanceGuard;

impl SelfBuilt for MaintenanceGuard {}

impl PreAuthGuard for MaintenanceGuard {
    fn check(
        &self,
        _ctx: &PreAuthGuardContext<'_>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async move {
            if is_maintenance_mode() {
                Err(HttpError::Custom {
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

## Guards that read beans

Guards are built **once, at controller registration**, from the resolved bean
graph — never per request, and there is no state access at request time. A guard
that needs a database pool (or any bean) holds it as a **field**, and a separate
**spec** type — named by the `#[guard(...)]` attribute expression — pulls the
beans out of the `BeanContext` in `build`. The bean
deps are declared at the type level in `Deps`, so a missing bean is a **compile
error at `register_controller()`** naming the type — exactly like a missing
`#[inject]` field.

```rust
use r2e::prelude::*; // Guard, GuardContext, Identity, DecoratorSpec, HttpError, IntoResponse, Response
use r2e::beans::BeanContext;
use r2e::type_list::{TCons, TNil};

// Spec: the value the attribute expression evaluates to. Reads no request data.
struct ActiveUserGuard;

// Product: the finished guard, holding the resolved beans as fields.
struct ActiveUserGuardReady {
    pool: SqlitePool,
}

impl DecoratorSpec for ActiveUserGuard {
    type Product = ActiveUserGuardReady;
    type Deps = TCons<SqlitePool, TNil>;   // beans build() pulls — compile-checked

    fn build(self, ctx: &BeanContext) -> ActiveUserGuardReady {
        ActiveUserGuardReady { pool: ctx.get::<SqlitePool>() }
    }
}

impl<I: Identity> Guard<I> for ActiveUserGuardReady {
    fn check(
        &self,
        ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async move {
            let sub = ctx.identity_sub().unwrap_or("");

            let active = sqlx::query_scalar::<_, bool>(
                "SELECT active FROM users WHERE sub = ?"
            )
            .bind(sub)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| HttpError::Internal("DB error".into()).into_response())?;

            match active {
                Some(true) => Ok(()),
                _ => Err(HttpError::Forbidden("Account suspended".into()).into_response()),
            }
        }
    }
}
```

Apply the spec by its type; the macro builds the product once and captures it in
the route closure:

```rust
#[get("/me")]
#[guard(ActiveUserGuard)]
async fn me(&self, #[inject(identity)] user: AuthenticatedUser) -> Json<User> { /* ... */ }
```

> If your guard is self-contained (no beans), skip the spec/product split and
> just `impl SelfBuilt for MyGuard {}` as shown above. For a free function or a
> local variable that produces the guard, use the escape hatch
> `#[guard(MyGuard = make_guard())]`, where the leading path names the spec type.

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
#[pre_guard(PreRateLimit::per_ip(10, 60))]    // IP rate limit (pre-auth)
#[roles("editor")]                             // role check
#[guard(TenantGuard)]                          // custom post-auth
#[guard(ActiveUserGuard)]                      // another post-auth (spec reads a bean)
async fn create(&self, body: Json<Request>) -> Json<Response> {
    // Reached only if ALL guards pass
}
```

Guards execute in order and short-circuit on first failure.
