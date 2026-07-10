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

With a struct-level `#[inject(identity)]` field, `#[roles]` needs no identity parameter — the guard reads the extracted struct identity directly. Routes opted out of authentication with `#[anonymous]` cannot carry `#[roles]` (compile error); other guards still run there with `identity: None`. See [Optional Identity](optional-identity.md).

## The `Guard` trait

Custom post-auth guards implement `Guard<I>`. A guard that reads no beans is a
self-contained decorator — implement the trait and add `impl SelfBuilt`:

```rust
use r2e::prelude::*; // Guard, GuardContext, Identity, SelfBuilt, IntoResponse, Response

struct TenantGuard;

impl SelfBuilt for TenantGuard {}

impl<I: Identity> Guard<I> for TenantGuard {
    fn check(
        &self,
        ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async move {
            match ctx.identity_claims() {
                Some(claims) if claims["tenant_id"].is_string() => Ok(()),
                _ => Err(HttpError::Forbidden("Missing tenant".into()).into_response()),
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
| `path_params` | `PathParams` | Route path parameters captured from patterns like `/projects/{pid}` |
| `identity` | `Option<&I>` | Authenticated identity (if available) |

Convenience methods: `identity_sub()`, `identity_email()`, `identity_claims()`, `path()`, `query_string()`, `path_param()`, `parse_path_param()`.

Use `parse_path_param()` for resource authorization guards that need typed IDs:

```rust
use r2e::prelude::*; // Guard, GuardContext, DecoratorSpec, GuardError, ...
use r2e::PathParam;
use r2e::beans::BeanContext;
use r2e::type_list::{TCons, TNil};
use std::future::Future;

#[derive(Clone, Copy)]
struct ProjectId(u64);

impl std::str::FromStr for ProjectId {
    type Err = std::num::ParseIntError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

#[derive(Clone, Copy)]
enum ProjectRole {
    Viewer,
}

// Spec: the value the `#[guard(...)]` expression evaluates to. Holds config
// (the path-param name + minimum role); reads the AuthzService bean in build().
// This one is hand-written (instead of `#[derive(DecoratorBean)]`) because the
// domain constructors (`viewer(path::pid)`, …) ARE the config surface — the
// derive's generated `spec(...)` would lose that vocabulary.
struct ProjectGuard {
    param: &'static str,
    min_role: ProjectRole,
}

impl ProjectGuard {
    const fn viewer(param: PathParam<ProjectId>) -> Self {
        Self {
            param: param.name(),
            min_role: ProjectRole::Viewer,
        }
    }
}

// Product: the finished guard, holding the resolved bean plus the config.
struct ProjectGuardReady {
    authz: AuthzService,
    param: &'static str,
    min_role: ProjectRole,
}

impl DecoratorSpec for ProjectGuard {
    type Product = ProjectGuardReady;
    type Deps = TCons<AuthzService, TNil>;   // compile-checked at register_controller()

    fn build(self, ctx: &BeanContext) -> ProjectGuardReady {
        ProjectGuardReady {
            authz: ctx.get::<AuthzService>(),
            param: self.param,
            min_role: self.min_role,
        }
    }
}

impl Guard<AuthenticatedUser> for ProjectGuardReady {
    fn check(
        &self,
        ctx: &GuardContext<'_, AuthenticatedUser>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async move {
            let user = ctx
                .identity
                .ok_or_else(|| GuardError::unauthorized("identity required"))?;
            let project_id: ProjectId = ctx.parse_path_param(self.param)?;

            // The authorization service was resolved once at registration.
            self.authz
                .require_project_role(user.sub(), project_id, self.min_role)
                .await
                .map_err(|_| Response::from(GuardError::forbidden("insufficient project role")))
        }
    }
}

#[get("/projects/{pid}")]
#[guard(ProjectGuard::viewer(path::pid))]
async fn project(
    &self,
    #[inject(identity)] user: AuthenticatedUser,
    Path(pid): Path<ProjectId>,
) -> Json<Project> {
    /* ... */
}
```

`#[routes]` generates a local `path` namespace for each guarded route. Each
symbol is a `PathParam<T>` containing the route parameter name and, when the
handler uses `Path<T>`, the extracted Rust type. String-based constructors such
as `ProjectGuard::viewer("pid")` continue to work for compatibility. Prefer
`path::pid` in new guard declarations because an unknown symbol like
`path::missing` fails at compile time. Use `ctx.parse_path_param::<T>("pid")`
inside the guard implementation when reading the actual request value.

Path-param parsing has consistent error mapping:

| Case | Response |
|------|----------|
| Guard references a missing route parameter | `500 Internal Server Error` |
| Route parameter is present but cannot parse as `T` | `400 Bad Request` |
| Authorization policy denies access | `403 Forbidden` |

For nested resources, keep the route parameter names explicit in the guard constructor:

```rust
#[get("/tenants/{tid}/projects/{pid}/sboms/{sid}")]
#[guard(TenantGuard::member("tid"))]
#[guard(ProjectGuard::viewer(path::pid))]
#[guard(SbomGuard::viewer(path::pid, path::sid))]
async fn sbom(
    &self,
    #[inject(identity)] user: AuthenticatedUser,
    Path((tid, pid, sid)): Path<(TenantId, ProjectId, SbomVersionId)>,
) -> Json<Sbom> {
    /* ... */
}
```

This pattern keeps controller routes readable while moving tenant/project policy decisions into application-level guards.

### The `Identity` trait

Guards are generic over the `Identity` trait, decoupling them from the concrete `AuthenticatedUser` type:

```rust
pub trait Identity: Send + Sync {
    fn sub(&self) -> &str;
    fn email(&self) -> Option<&str> { None }
    fn claims(&self) -> Option<&serde_json::Value> { None }
}
```

`AuthenticatedUser` implements `Identity`. Role checks use `RoleBasedIdentity` from `r2e-security`. You can create custom identity types by implementing these traits.

## Pre-auth guards

For authorization that doesn't need identity (e.g., IP allowlisting), use `PreAuthGuard`:

```rust
use r2e::prelude::*; // PreAuthGuard, PreAuthGuardContext, SelfBuilt, HttpError, IntoResponse, Response

struct IpAllowlistGuard;

impl SelfBuilt for IpAllowlistGuard {}

impl PreAuthGuard for IpAllowlistGuard {
    fn check(
        &self,
        ctx: &PreAuthGuardContext<'_>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async move {
            let ip = ctx.headers.get("x-forwarded-for")
                .and_then(|v| v.to_str().ok());
            match ip {
                Some("10.0.0.1") => Ok(()),
                _ => Err(HttpError::Forbidden("IP not allowed".into()).into_response()),
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

A guard that needs a database pool holds it as an `#[inject]` field;
`#[derive(DecoratorBean)]` pulls the bean from the graph once, at registration:

```rust
#[derive(DecoratorBean)]
struct DatabaseGuard {
    #[inject]
    pool: sqlx::SqlitePool,
}

impl<I: Identity> Guard<I> for DatabaseGuard {
    fn check(
        &self,
        ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async move {
            let allowed = sqlx::query_scalar::<_, bool>(
                "SELECT active FROM users WHERE sub = ?"
            )
            .bind(ctx.identity_sub().unwrap_or(""))
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| HttpError::Internal("DB error".into()).into_response())?;

            match allowed {
                Some(true) => Ok(()),
                _ => Err(HttpError::Forbidden("Account suspended".into()).into_response()),
            }
        }
    }
}
```

Applied with `#[guard(DatabaseGuard::spec())]` — the generated constructor
takes the non-injected fields (none here).

> The database query still runs **per request** inside `check`. What changed is
> that the pool is resolved **once** at registration and held as a field — there
> is no `state.bean::<...>()` lookup at request time.

## Guard execution order

1. Pre-auth guards (`#[pre_guard(...)]`) — run before JWT validation
2. Rate limit guards (`RateLimit::per_user(...)`) — run after JWT validation
3. Role guards (`#[roles("...")]`) — check identity roles
4. Custom guards (`#[guard(...)]`) — run after roles

Guards short-circuit on first failure — later guards don't run.

## Combining guards

```rust
#[post("/")]
#[pre_guard(PreRateLimit::per_ip(10, 60))]    // Pre-auth: IP rate limit
#[guard(RateLimit::per_user(5, 60))]          // Post-auth: user rate limit
#[roles("editor")]                             // Role check
#[guard(TenantGuard)]                          // Custom check
async fn create(&self, body: Json<Request>) -> Json<Response> {
    // Only reached if ALL guards pass
}
```

## Combining guards with interceptors

Guards and interceptors work together. Guards run first and short-circuit independently; interceptors see the handler's raw return type, not `Response`:

```rust
#[get("/admin/users")]
#[roles("admin")]
#[intercept(Cache::ttl(30).group("admin_users"))]
async fn admin_list(&self) -> Json<Vec<User>> {
    // Cache interceptor sees Json<Vec<User>>, roles guard runs before
}
```
