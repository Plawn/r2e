# Feature 4 — Interceptors

## TL;DR

A single `#[intercept(Spec)]` attribute wraps controller-method behavior via the `Interceptor<R>` around-pattern — built once at registration (graph-resolved, zero per-request cost, no `dyn`). Built-in specs (from `r2e-utils`): `Logged`, `Timed`, `Cache`, `CacheInvalidate`, `Counted`, `MetricTimed` — e.g. `#[intercept(Logged::info())]`, `#[intercept(Timed::threshold(100))]`, `#[intercept(Cache::ttl(30).group("users"))]`, `#[intercept(CacheInvalidate::group("users"))]`. Custom ones apply the same way (impl `Interceptor<R>` + `SelfBuilt` or `DecoratorSpec`). Rate limiting is a **guard**, not an interceptor: `#[guard(RateLimit::per_user(N, S))]` (post-auth) / `#[pre_guard(PreRateLimit::global(N, S))]` / `#[pre_guard(PreRateLimit::per_ip(N, S))]` (pre-auth). Order: pre-auth guards → guards/`#[roles]` → controller-level then method-level interceptors → body. Interceptors always see the raw return type, never `Response`.


## Goal

Provide declarative attributes to enrich controller method behavior: logging, performance measurement, caching, rate limiting, transactions, and user-defined custom interceptors.

## Architecture

Interceptors are based on a generic `Interceptor<R>` trait with an `around` pattern (defined in `r2e-core/src/interceptors.rs`). `R` is the wrapped return type. Interceptors are **graph-resolved decorators**: they are built **once, at controller registration**, from the resolved `BeanContext` — never per request — and any beans they read are held as fields. Built-in interceptor specs (`Logged`, `Timed`, `Cache`, `CacheInvalidate`, `Counted`, `MetricTimed`) live in `r2e-utils` (`r2e-utils/src/interceptors.rs`); self-contained ones opt in with `impl SelfBuilt`, while bean-reading ones (`Cache`, `CacheInvalidate`) implement `DecoratorSpec`. All calls are monomorphized (no `dyn`) — zero-cost at runtime. There is **no state access at request time**.

Every interceptor — built-in or custom — is applied the **same way**, with `#[intercept(Spec)]`. There are no dedicated per-effect attributes.

Rate limiting is **not** an interceptor: it is a **guard** (short-circuits before the body, like `#[roles]`), applied with `#[guard(RateLimit::...)]` (post-auth) or `#[pre_guard(PreRateLimit::...)]` (pre-auth, before JWT extraction). See the [Rate limiting](#rate-limiting) section.

### Why no-op attributes?

The attributes parsed by `#[routes]` (`#[intercept]`, `#[guard]`, `#[pre_guard]`, `#[roles]`, …) are declared in `r2e-macros/src/lib.rs` as no-op `#[proc_macro_attribute]` — they return their input without transformation. The actual logic is in the `#[routes]` attribute, which parses these attributes from the raw token stream of the `impl` block.

These no-op declarations exist for three reasons:

1. **Avoiding compiler errors** — without a declaration, using `#[intercept]` outside of `#[routes]` (by mistake or during refactoring) would cause `cannot find attribute "intercept"`.
2. **Discoverability** — the attributes appear in `cargo doc` with their documentation, making the API explicit.
3. **IDE support** — rust-analyzer and other tools provide autocompletion and hover documentation for registered attributes.

## Overview

Every built-in effect is a spec type applied via `#[intercept(...)]` (or, for rate limiting, `#[guard(...)]`/`#[pre_guard(...)]`):

| Attribute | Effect | Prerequisites |
|-----------|--------|---------------|
| `#[intercept(Logged::info())]` | Logs `entering`/`exiting` via `Interceptor` trait | None |
| `#[intercept(Logged::debug())]` / `Logged::level(LogLevel::Debug)` | Same, configurable level | None |
| `#[intercept(Timed::new())]` | Logs execution time | None |
| `#[intercept(Timed::threshold(100))]` | Logs only if > 100ms | None |
| `#[intercept(Cache::ttl(N))]` | Caches the result for N seconds | Return type impl `Cacheable` (e.g. `Json<T>`) + `CacheStore` bean |
| `#[intercept(Cache::ttl(N).group("x"))]` | Named cache (for invalidation) | Same |
| `#[intercept(Cache::with_key(N, key))]` | Explicit per-call key | Same |
| `#[intercept(CacheInvalidate::group("x"))]` | Invalidates a cache group after execution | `CacheStore` bean |
| `#[intercept(Counted::new("m"))]` / `MetricTimed::new("m")` | Named counter / duration metric | None |
| `#[guard(RateLimit::per_user(N, S))]` | Per-user request limit | `RateLimitRegistry` bean + identity |
| `#[pre_guard(PreRateLimit::global(N, S))]` | Global request limit | `RateLimitRegistry` bean |
| `#[pre_guard(PreRateLimit::per_ip(N, S))]` | Per-IP-address limit | `RateLimitRegistry` bean + `X-Forwarded-For` header |
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

## `Logged`

Adds traces on entry and exit via the `Interceptor` trait. Applied with `#[intercept(Logged::...)]` — `Logged` is `SelfBuilt` (no bean deps).

```rust
#[get("/users")]
#[intercept(Logged::info())]                   // default: Info
async fn list(&self) -> axum::Json<Vec<User>> { ... }

#[get("/users")]
#[intercept(Logged::debug())]                  // custom level
async fn list(&self) -> axum::Json<Vec<User>> { ... }
```

Level constructors: `Logged::trace()`, `Logged::debug()`, `Logged::info()`, `Logged::warn()`, `Logged::error()`, or `Logged::level(LogLevel::Debug)`.

Generated logs (info level):

```
INFO method="list" "entering"
INFO method="list" "exiting"
```

## `Timed`

Measures execution time. With an optional threshold, logs only if the time exceeds the threshold. `Timed` is `SelfBuilt`.

```rust
#[get("/users")]
#[intercept(Timed::new())]                     // default: Info, no threshold
async fn list(&self) -> axum::Json<Vec<User>> { ... }

#[get("/users")]
#[intercept(Timed::threshold_warn(100))]       // Warn, only if > 100ms
async fn list(&self) -> axum::Json<Vec<User>> { ... }
```

Constructors: `Timed::new()` / `Timed::info()` / `Timed::debug()` / `Timed::warn()` (no threshold), `Timed::threshold(ms)` (Info, only if exceeded), `Timed::threshold_warn(ms)` (Warn, only if exceeded).

Generated log (without threshold or threshold exceeded):

```
INFO method="list" "elapsed_ms=3"
```

### Combining `Logged` + `Timed`

```rust
#[get("/users")]
#[intercept(Logged::debug())]
#[intercept(Timed::threshold(50))]
async fn list(&self) -> axum::Json<Vec<User>> { ... }
```

Logs (if the request takes more than 50ms):

```
DEBUG method="list" "entering"
INFO  method="list" "elapsed_ms=73"
DEBUG method="list" "exiting"
```

## `Cache`

Caches the method result. `Cache` is a `DecoratorSpec`: it reads an `Arc<dyn CacheStore>` bean from the graph (a missing store is a compile error at `register_controller()`) and holds it in the built `CacheInterceptor`.

### Provide a store

```rust
use r2e_cache::InMemoryStore;

AppBuilder::new()
    .provide(InMemoryStore::shared())   // Arc<dyn CacheStore>
    // ...
```

### Syntax

```rust
#[intercept(Cache::ttl(30))]                                  // default key (controller_method:default)
#[intercept(Cache::ttl(30).group("users"))]                  // named group (for invalidation)
#[intercept(Cache::with_key(30, format!("user:{}", id)))]    // explicit per-call key
```

### Constraints

- The return type must implement `Cacheable`:
  - `Json<T>` where `T: Serialize + DeserializeOwned + Send`
  - `Result<T, E>` where `T: Cacheable, E: Send` (only `Ok` values are cached)
  - Types with `#[derive(Cacheable)]`
- The cache serializes/deserializes via JSON using `serde_json`
- A `CacheStore` bean must be provided (e.g. `InMemoryStore::shared()`)

The cache key is `full_key(controller, method)`: with no `group`, it is `__{controller}_{method}:{key-or-"default"}`; with a `group`, the prefix becomes the group name (`{group}:{key-or-"default"}`), which is what `CacheInvalidate` targets.

### Cache groups and invalidation

`CacheInvalidate` is also a `DecoratorSpec` (reads the same `CacheStore` bean). After the body runs, it removes every entry whose key starts with `{group}:`.

```rust
#[get("/users")]
#[intercept(Cache::ttl(30).group("users"))]
async fn list(&self) -> axum::Json<Vec<User>> { ... }

#[post("/users")]
#[intercept(CacheInvalidate::group("users"))]
async fn create(&self, ...) -> axum::Json<User> { ... }
```

### Internal mechanism

```
Request → Interceptor::around(&cache, ctx, next)
            ├── store.get(key)
            │     ├── Hit → R::from_cache → return value
            │     └── Deserialization failed → store.remove(key) → fallthrough
            └── Miss → next().await → R::to_cache → store.set(key, bytes, ttl) → return
```

## Rate limiting

Rate limiting is a **guard**, not an interceptor — it short-circuits with a 429 before the body runs. The specs live in `r2e-rate-limit` and read a `RateLimitRegistry` bean (provide one, e.g. `.provide(RateLimitRegistry::default())`).

- **Post-authentication, per user** — `#[guard(RateLimit::per_user(max, window_secs))]` (each authenticated subject gets its own bucket; runs after JWT validation).
- **Pre-authentication, global** — `#[pre_guard(PreRateLimit::global(max, window_secs))]` (one shared bucket; runs before JWT extraction).
- **Pre-authentication, per IP** — `#[pre_guard(PreRateLimit::per_ip(max, window_secs))]` (bucket per `X-Forwarded-For` client, first element trimmed, `"unknown"` fallback).

### Syntax

```rust
#[pre_guard(PreRateLimit::global(5, 60))]        // 5 req / 60s, global
#[pre_guard(PreRateLimit::per_ip(5, 60))]        // 5 req / 60s, per IP
#[guard(RateLimit::per_user(5, 60))]             // 5 req / 60s, per user
```

### Response on rate limit exceeded

```http
HTTP/1.1 429 Too Many Requests
Content-Type: application/json

{"error": "Rate limit exceeded"}
```

### Internal mechanism

The backing `RateLimiter<K>` uses a **token bucket** algorithm:
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
#[intercept(Logged::info())]
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
    #[intercept(Logged::debug())]
    #[intercept(Timed::threshold(50))]
    async fn list(&self) -> axum::Json<Vec<User>> {
        axum::Json(self.user_service.list().await)
    }

    // Cache group + invalidation
    #[get("/cached")]
    #[intercept(Cache::ttl(30).group("users"))]
    #[intercept(Timed::new())]
    async fn cached_list(&self) -> axum::Json<serde_json::Value> {
        let users = self.user_service.list().await;
        axum::Json(serde_json::to_value(users).unwrap())
    }

    #[post("/")]
    #[intercept(CacheInvalidate::group("users"))]
    async fn create(&self, axum::Json(body): axum::Json<CreateUserRequest>) -> axum::Json<User> {
        axum::Json(self.user_service.create(body.name, body.email).await)
    }

    // Rate limit per user (a guard, not an interceptor)
    #[post("/rate-limited")]
    #[guard(RateLimit::per_user(5, 60))]
    async fn create_rate_limited(&self, axum::Json(body): axum::Json<CreateUserRequest>)
        -> axum::Json<User>
    {
        axum::Json(self.user_service.create(body.name, body.email).await)
    }

    // Custom interceptor
    #[get("/audited")]
    #[intercept(Logged::info())]
    #[intercept(AuditLog)]
    async fn audited_list(&self) -> axum::Json<Vec<User>> {
        axum::Json(self.user_service.list().await)
    }
}
```

This controller needs an `Arc<dyn CacheStore>` bean (for `Cache`/`CacheInvalidate`)
and a `RateLimitRegistry` bean (for `RateLimit`) provided on the builder.

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
