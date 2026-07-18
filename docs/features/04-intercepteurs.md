# Feature 4 — Interceptors

## TL;DR

Declarative attributes that wrap controller-method behavior via the `Interceptor<R>` around-pattern — built once at registration (graph-resolved, zero per-request cost, no `dyn`). Built-ins: `#[logged]`, `#[timed]`, `#[cached(ttl = N)]`, `#[cache_invalidate("group")]`, `#[rate_limited(max = N, window = S)]`. Custom ones apply with `#[intercept(Type)]` (impl `Interceptor<R>` + `SelfBuilt` or `DecoratorSpec`). Order: pre-auth guards → guards/`#[roles]` → controller-level then method-level interceptors → body. Interceptors always see the raw return type, never `Response`.


## Goal

Provide declarative attributes to enrich controller method behavior: logging, performance measurement, caching, rate limiting, transactions, and user-defined custom interceptors.

## Architecture

Interceptors are based on a generic `Interceptor<R>` trait with an `around` pattern (defined in `r2e-core/src/interceptors.rs`). `R` is the wrapped return type. Interceptors are **graph-resolved decorators** (Phase 6): they are built **once, at controller registration**, from the resolved `BeanContext` — never per request — and any beans they read are held as fields. Built-in interceptors (`Logged`, `Timed`, `Cache`) are structs that implement this trait; self-contained ones opt in with `impl SelfBuilt`, while bean-reading ones implement `DecoratorSpec`. All calls are monomorphized (no `dyn`) — zero-cost at runtime. There is **no state access at request time**.

Exceptions to this architecture:
- **`rate_limited`** — handled at the handler level (short-circuits before the controller, like `#[roles]`)
- **`cache_invalidate`** — pure codegen (call after the body)

User-defined interceptors implement the same trait and are applied via `#[intercept(TypeName)]`.

### Why no-op attributes?

All interceptor attributes (`#[logged]`, `#[timed]`, `#[cached]`, `#[rate_limited]`, `#[cache_invalidate]`, `#[intercept]`) are declared in `r2e-macros/src/lib.rs` as no-op `#[proc_macro_attribute]` — they return their input without transformation. The actual logic is in the `#[routes]` attribute, which parses these attributes from the raw token stream of the `impl` block.

These no-op declarations exist for three reasons:

1. **Avoiding compiler errors** — without a declaration, using `#[logged]` outside of `#[routes]` (by mistake or during refactoring) would cause `cannot find attribute "logged"`.
2. **Discoverability** — the attributes appear in `cargo doc` with their documentation, making the API explicit.
3. **IDE support** — rust-analyzer and other tools provide autocompletion and hover documentation for registered attributes.

## Overview

| Attribute | Effect | Prerequisites |
|-----------|--------|---------------|
| `#[logged]` | Logs `entering`/`exiting` via `Interceptor` trait | None |
| `#[logged(level = "debug")]` | Same, configurable level | None |
| `#[timed]` | Logs execution time | None |
| `#[timed(threshold = 100)]` | Logs only if > 100ms | None |
| `#[cached(ttl = N)]` | Caches the result for N seconds | Return type `axum::Json<T>` or `T: Serialize + DeserializeOwned` |
| `#[cached(ttl = N, group = "x")]` | Named cache (for invalidation) | Same |
| `#[cached(ttl = N, key = "params")]` | Key based on parameters | Parameters impl `Debug` |
| `#[cache_invalidate("x")]` | Invalidates a cache group after execution | None |
| `#[rate_limited(max = N, window = S)]` | Global request limit | None |
| `#[rate_limited(..., key = "user")]` | Per-user limit | `#[inject(identity)]` field |
| `#[rate_limited(..., key = "ip")]` | Per-IP-address limit | `X-Forwarded-For` header |
| `#[intercept(Type)]` | User-defined custom interceptor | Type impl `Interceptor<R>` (+ `SelfBuilt` or `DecoratorSpec`) |

### Application order

Interceptors are applied in a fixed order, from outermost to innermost:

```
Pre-auth middleware level (before JWT extraction):
  → pre_guard (PreRateLimit::global, PreRateLimit::per_ip, custom PreAuthGuard)

Handler level (after extraction, before body):
  → guard (RateLimit::per_user, custom Guard)
  → roles — short-circuit 403

Body level (Interceptor::around trait):
  → controller-level interceptors (declaration order)
  → method-level interceptors (declaration order)
  → method body
```

**Design invariant:** Interceptors always see the **raw return type** of the handler (`Json<T>`, `Result<Json<T>, E>`, etc.), never `Response`. The `IntoResponse::into_response()` conversion is applied *after* the outermost interceptor. Guards short-circuit *before* the interceptors and do not affect the type seen by interceptors.

### Combining interceptors + guards/roles

`#[intercept(Cache)]` + `#[roles]` (or any `#[guard]`) works correctly:
```rust
#[get("/admin/users")]
#[roles("admin")]
#[intercept(Cache::ttl(30).group("admin_users"))]
async fn admin_list(&self) -> Json<Vec<User>> { /* ... */ }
```

**Known limitation:** `#[managed]` + `#[intercept(Cache)]` does NOT work — the managed resource lifecycle (acquire/release with error handling) wraps `into_response` inside the interceptor closure, so `Cache` sees `Response` instead of the raw type. Workaround: inject the store bean (`#[inject] store: Arc<dyn CacheStore>`) and cache manually in the handler body.

## The `Interceptor<R>` trait

```rust
/// Context passed to each interceptor. Carries handler identification only —
/// `Copy`, no state field. Interceptors that need services hold them as fields.
#[derive(Clone, Copy)]
pub struct InterceptorContext {
    pub method_name: &'static str,
    pub controller_name: &'static str,
}

/// Trait generic over the wrapped return type `R`.
pub trait Interceptor<R> {
    fn around<F, Fut>(&self, ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send;
}
```

Interceptors no longer receive the application state — there is no state access
at request time. Instead, a decorator is built **once at registration** via the
`DecoratorSpec` contract, and any beans it needs are pulled from the graph and
held as fields:

- An interceptor that never reads a bean is **self-contained**: implement
  `Interceptor<R>` and opt in with `impl SelfBuilt for AuditLog {}`.
- An interceptor that **reads a bean** holds it as a field, and a **spec** type
  (named by the `#[intercept(...)]` expression) pulls it in `build`:
  `impl DecoratorSpec for DbAudit { type Product = DbAuditReady; type Deps = TCons<SqlitePool, TNil>; fn build(self, ctx) -> DbAuditReady { DbAuditReady { pool: ctx.get() } } }`.
  A missing bean is a **compile error at `register_controller()`** naming the type.

`SelfBuilt`, `DecoratorSpec`, `BeanContext`, and `InterceptorContext` are all in
the prelude (`TCons`/`TNil` live in `r2e::type_list`). `ctx.method_name` and
`ctx.controller_name` are `Copy`, so they can be captured by each nested
`async move` closure.

## `#[logged]`

Adds traces on entry and exit via the `Interceptor` trait.

```rust
#[get("/users")]
#[logged]                        // default: Info
async fn list(&self) -> axum::Json<Vec<User>> { ... }

#[get("/users")]
#[logged(level = "debug")]       // custom level
async fn list(&self) -> axum::Json<Vec<User>> { ... }
```

Available levels: `trace`, `debug`, `info`, `warn`, `error`.

Generated logs (info level):

```
INFO method="list" "entering"
INFO method="list" "exiting"
```

## `#[timed]`

Measures execution time. With an optional threshold, logs only if the time exceeds the threshold.

```rust
#[get("/users")]
#[timed]                                     // default: Info, no threshold
async fn list(&self) -> axum::Json<Vec<User>> { ... }

#[get("/users")]
#[timed(level = "warn", threshold = 100)]    // only if > 100ms
async fn list(&self) -> axum::Json<Vec<User>> { ... }
```

Generated log (without threshold or threshold exceeded):

```
INFO method="list" "elapsed_ms=3"
```

### Combining `#[logged]` + `#[timed]`

```rust
#[get("/users")]
#[logged(level = "debug")]
#[timed(threshold = 50)]
async fn list(&self) -> axum::Json<Vec<User>> { ... }
```

Logs (if the request takes more than 50ms):

```
DEBUG method="list" "entering"
INFO  method="list" "elapsed_ms=73"
DEBUG method="list" "exiting"
```

## `#[cached]`

Caches the method result. The cache uses the `Interceptor<axum::Json<T>>` trait where `T: Serialize + DeserializeOwned`.

### Syntax

```rust
#[cached(ttl = 30)]                          // anonymous cache, default key
#[cached(ttl = 30, group = "users")]         // named cache (for invalidation)
#[cached(ttl = 30, key = "params")]          // key based on parameters
#[cached(ttl = 30, key = "user")]            // key per user (identity.sub)
#[cached(ttl = 30, key = "user_params")]     // combination user + params
```

### Constraints

- The return type must implement `Cacheable`:
  - `Json<T>` where `T: Serialize + DeserializeOwned + Send`
  - `Result<T, E>` where `T: Cacheable, E: Send` (only `Ok` values are cached)
  - Types with `#[derive(Cacheable)]`
- The cache serializes/deserializes via JSON using `serde_json`
- For `key = "params"`, the method parameters must implement `Debug`
- For `key = "user"` or `key = "user_params"`, the controller must have an `#[inject(identity)]` field

### Cache groups and invalidation

```rust
#[get("/users")]
#[cached(ttl = 30, group = "users")]
async fn list(&self) -> axum::Json<Vec<User>> { ... }

#[post("/users")]
#[cache_invalidate("users")]
async fn create(&self, ...) -> axum::Json<User> { ... }
```

The `CacheRegistry` (global static in `r2e-core/src/cache.rs`) maintains a registry of named caches:
- `get_or_create(group, ttl)` — returns the cache for the group (creates it on first call)
- `invalidate(group)` — clears the cache for the group

**Note**: the TTL is determined by the first call to `get_or_create`. If two methods refer to the same group with different TTLs, the first one to execute sets the TTL.

### Internal mechanism

```
Request → Interceptor::around(&cached, ctx, next)
            ├── cache.get(key)
            │     ├── Hit → deserialize → return Json<T>
            │     └── Deserialization failed → cache.remove(key) → fallthrough
            └── Miss → next().await → serialize → cache.insert(key) → return
```

## `#[rate_limited]`

Limits the number of requests. Handled at the **handler level** (short-circuits before the controller).

### Syntax

```rust
#[rate_limited(max = 5, window = 60)]                   // global
#[rate_limited(max = 5, window = 60, key = "user")]      // per user
#[rate_limited(max = 5, window = 60, key = "ip")]        // per IP address
```

### Key strategies

| Key | Generated code | Prerequisites |
|-----|---------------|---------------|
| `"global"` (default) | `format!("{}:global", fn_name)` | None |
| `"user"` | `format!("{}:user:{}", fn_name, identity.sub)` | `#[inject(identity)]` field |
| `"ip"` | `format!("{}:ip:{}", fn_name, ip)` | `X-Forwarded-For` header |

For `key = "ip"`, the IP is extracted from the `X-Forwarded-For` header (first element, trimmed). Fallback: `"unknown"`.

### Constraints

- The generated handler returns `axum::response::Response` (like `#[roles]`) to allow the 429 short-circuit
- The method return type **no longer needs** to be `Result<T, HttpError>` — any `IntoResponse` type works
- The rate limiter is a `static OnceLock<RateLimiter<String>>` per handler

### Response on rate limit exceeded

```http
HTTP/1.1 429 Too Many Requests
Content-Type: application/json

{"error": "Rate limit exceeded"}
```

### Internal mechanism

The `RateLimiter<K>` uses a **token bucket** algorithm:
- Each key has a bucket of `max` tokens
- Tokens refill linearly over the `window`
- Each request consumes 1 token
- If the bucket is empty → 429 rejection

## `#[intercept(Type)]` — User-defined interceptors

Users can create their own interceptors by implementing the `Interceptor<R>` trait. A self-contained interceptor (no bean deps) stays generic over `R` and opts in with `impl SelfBuilt`:

```rust
pub struct AuditLog;

impl r2e_core::SelfBuilt for AuditLog {}

impl<R: Send> r2e_core::Interceptor<R> for AuditLog {
    fn around<F, Fut>(
        &self,
        ctx: r2e_core::InterceptorContext,
        next: F,
    ) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        let method_name = ctx.method_name;
        async move {
            tracing::info!(method = method_name, "audit: entering");
            let result = next().await;
            tracing::info!(method = method_name, "audit: done");
            result
        }
    }
}
```

An interceptor that reads a bean holds it as a **field**; a separate **spec** type — named by the `#[intercept(...)]` expression — pulls the bean from the graph in `build`, once at registration:

```rust
use r2e_core::{DecoratorSpec, Interceptor, InterceptorContext, BeanContext};
use r2e_core::type_list::{TCons, TNil};

// Spec: the value the attribute expression evaluates to.
pub struct DbAudit;

// Product: the finished interceptor, holding the resolved bean.
pub struct DbAuditReady {
    pool: sqlx::SqlitePool,
}

impl DecoratorSpec for DbAudit {
    type Product = DbAuditReady;
    type Deps = TCons<sqlx::SqlitePool, TNil>;   // compile-checked at register_controller()

    fn build(self, ctx: &BeanContext) -> DbAuditReady {
        DbAuditReady { pool: ctx.get::<sqlx::SqlitePool>() }
    }
}

impl<R: Send> Interceptor<R> for DbAuditReady {
    fn around<F, Fut>(
        &self,
        ctx: InterceptorContext,
        next: F,
    ) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        let method_name = ctx.method_name;
        let pool = self.pool.clone();   // resolved once at registration
        async move {
            let result = next().await;
            let _ = sqlx::query("INSERT INTO audit_log (method, ts) VALUES (?, datetime('now'))")
                .bind(method_name)
                .execute(&pool)
                .await;
            result
        }
    }
}
```

Apply it with `#[intercept(DbAudit)]` — the macro builds `DbAuditReady` once and captures it in the route closure.

Usage:

```rust
#[get("/users/audited")]
#[logged]
#[intercept(AuditLog)]
async fn audited_list(&self) -> axum::Json<Vec<User>> { ... }
```

### Constraints

- The `#[intercept(...)]` expression's leading type path names the decorator spec. Builder-style call syntax works (`#[intercept(Cache::ttl(30).group("x"))]`); for a self-contained interceptor the type is the interceptor itself (`impl SelfBuilt`). Use the escape hatch `#[intercept(MyIcept = make_icept())]` for a free function or variable.
- The interceptor is generic over `R`. If it reads beans, hold them as fields and implement `DecoratorSpec` on a config type (Product + Deps + build).

## Complete example

```rust
use std::future::Future;
use r2e_core::prelude::*;

/// Custom interceptor (self-contained: no bean deps)
pub struct AuditLog;

impl SelfBuilt for AuditLog {}

impl<R: Send> Interceptor<R> for AuditLog {
    fn around<F, Fut>(&self, ctx: InterceptorContext, next: F)
        -> impl Future<Output = R> + Send
    where F: FnOnce() -> Fut + Send, Fut: Future<Output = R> + Send,
    {
        let method_name = ctx.method_name;
        async move {
            tracing::info!(method = method_name, "audit: entering");
            let result = next().await;
            tracing::info!(method = method_name, "audit: done");
            result
        }
    }
}

#[controller(path = "/users")]
pub struct UserController {
    #[inject]
    user_service: UserService,

    #[inject]
    pool: sqlx::SqlitePool,

    #[inject(identity)]
    user: AuthenticatedUser,
}

#[routes]
impl UserController {
    // Logged debug + timed with threshold
    #[get("/")]
    #[logged(level = "debug")]
    #[timed(threshold = 50)]
    async fn list(&self) -> axum::Json<Vec<User>> {
        axum::Json(self.user_service.list().await)
    }

    // Cache group + invalidation
    #[get("/cached")]
    #[cached(ttl = 30, group = "users")]
    #[timed]
    async fn cached_list(&self) -> axum::Json<serde_json::Value> {
        let users = self.user_service.list().await;
        axum::Json(serde_json::to_value(users).unwrap())
    }

    #[post("/")]
    #[cache_invalidate("users")]
    async fn create(&self, axum::Json(body): axum::Json<CreateUserRequest>) -> axum::Json<User> {
        axum::Json(self.user_service.create(body.name, body.email).await)
    }

    // Rate limit per user
    #[post("/rate-limited")]
    #[rate_limited(max = 5, window = 60, key = "user")]
    async fn create_rate_limited(&self, axum::Json(body): axum::Json<CreateUserRequest>)
        -> axum::Json<User>
    {
        axum::Json(self.user_service.create(body.name, body.email).await)
    }

    // Custom interceptor
    #[get("/audited")]
    #[logged]
    #[intercept(AuditLog)]
    async fn audited_list(&self) -> axum::Json<Vec<User>> {
        axum::Json(self.user_service.list().await)
    }
}
```

## Validation criteria

```bash
# Cached with group — two rapid calls, the second comes from cache
curl -H "Authorization: Bearer <token>" http://localhost:3000/users/cached
curl -H "Authorization: Bearer <token>" http://localhost:3000/users/cached

# Cache invalidation — create clears the cache
curl -X POST http://localhost:3000/users \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"name":"New","email":"new@example.com"}'
curl -H "Authorization: Bearer <token>" http://localhost:3000/users/cached
# → contains the new user

# Rate limited per-user — after 5 requests, the 6th returns 429
for i in $(seq 1 6); do
  curl -s -o /dev/null -w "%{http_code}\n" \
    -X POST http://localhost:3000/users/rate-limited \
    -H "Authorization: Bearer <token>" \
    -H "Content-Type: application/json" \
    -d '{"name":"Test","email":"test@example.com"}'
done
# → 200 200 200 200 200 429

# Two distinct users have independent counters (key = "user")

# Custom interceptor — audit log visible in the output
curl -H "Authorization: Bearer <token>" http://localhost:3000/users/audited
# → logs: audit: entering / audit: done
```
