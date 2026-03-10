# Feature 4 — Interceptors

## Goal

Provide declarative attributes to enrich controller method behavior: logging, performance measurement, caching, rate limiting, transactions, and user-defined custom interceptors.

## Architecture

Interceptors are based on a generic `Interceptor<R>` trait with an `around` pattern (defined in `r2e-core/src/interceptors.rs`). Built-in interceptors (`Logged`, `Timed`, `Cached`) are structs that implement this trait. All calls are monomorphized (no `dyn`) — zero-cost at runtime.

Exceptions to this architecture:
- **`rate_limited`** — handled at the handler level (short-circuits before the controller, like `#[roles]`)
- **`transactional`** — pure codegen (injection of the `tx` variable into the body)
- **`cache_invalidate`** — pure codegen (call after the body)

User-defined interceptors implement the same trait and are applied via `#[intercept(TypeName)]`.

### Why no-op attributes?

All interceptor attributes (`#[logged]`, `#[timed]`, `#[cached]`, `#[rate_limited]`, `#[transactional]`, `#[cache_invalidate]`, `#[intercept]`) are declared in `r2e-macros/src/lib.rs` as no-op `#[proc_macro_attribute]` — they return their input without transformation. The actual logic is in the `#[routes]` attribute, which parses these attributes from the raw token stream of the `impl` block.

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
| `#[rate_limited(..., key = "user")]` | Per-user limit | `#[identity]` field |
| `#[rate_limited(..., key = "ip")]` | Per-IP-address limit | `X-Forwarded-For` header |
| `#[transactional]` | SQL transaction with auto-commit/rollback | Injected `pool` field |
| `#[transactional(pool = "read_db")]` | Transaction on a specific pool | Corresponding injected field |
| `#[intercept(Type)]` | User-defined custom interceptor | Type impl `Interceptor<R>` |

### Application order

Interceptors are applied in a fixed order, from outermost to innermost:

```
Pre-auth middleware level (before JWT extraction):
  → pre_guard (RateLimit::global, RateLimit::per_ip, custom PreAuthGuard)

Handler level (after extraction, before body):
  → guard (RateLimit::per_user, custom Guard)
  → roles — short-circuit 403

Body level (Interceptor::around trait):
  → controller-level interceptors (declaration order)
  → method-level interceptors (declaration order)

Pure codegen (inline wrapping):
  → transactional (tx injection)
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

**Known limitation:** `#[managed]` + `#[intercept(Cache)]` does NOT work — the managed resource lifecycle (acquire/release with error handling) wraps `into_response` inside the interceptor closure, so `Cache` sees `Response` instead of the raw type. Workaround: use `cache_backend()` manually in the handler body.

## The `Interceptor<R>` trait

```rust
/// Context passed to each interceptor. Copy for capture by async move closures.
#[derive(Clone, Copy)]
pub struct InterceptorContext {
    pub method_name: &'static str,
    pub controller_name: &'static str,
}

/// Trait generic over return type R.
pub trait Interceptor<R> {
    fn around<F, Fut>(&self, ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send;
}
```

`InterceptorContext` is `Copy`, which allows it to be captured by each nested `async move` closure without ownership issues.

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
- For `key = "user"` or `key = "user_params"`, the controller must have an `#[identity]` field

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
| `"user"` | `format!("{}:user:{}", fn_name, identity.sub)` | `#[identity]` field |
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

## `#[transactional]`

Wraps the method body in a SQL transaction.

```rust
#[post("/users/db")]
#[transactional]                             // default: self.pool
async fn create_in_db(&self, ...) -> Result<axum::Json<User>, r2e_core::HttpError> {
    sqlx::query("INSERT ...").execute(&mut *tx).await?;
    Ok(axum::Json(user))
}

#[transactional(pool = "read_db")]           // specific pool
async fn read_data(&self, ...) -> Result<...> { ... }
```

### Constraints

- The controller must have an `#[inject]` field for the specified pool (default: `pool`)
- The body can use `tx` (variable injected by the macro, of type `Transaction`)
- The return type **must** be `Result<T, HttpError>`

## `#[intercept(Type)]` — User-defined interceptors

Users can create their own interceptors by implementing the `Interceptor<R>` trait:

```rust
pub struct AuditLog;

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
        async move {
            tracing::info!(method = ctx.method_name, "audit: entering");
            let result = next().await;
            tracing::info!(method = ctx.method_name, "audit: done");
            result
        }
    }
}
```

Usage:

```rust
#[get("/users/audited")]
#[logged]
#[intercept(AuditLog)]
async fn audited_list(&self) -> axum::Json<Vec<User>> { ... }
```

### Constraints

- The type passed to `#[intercept(...)]` must be constructible as a path expression (unit struct or constant). Call syntax (`#[intercept(Foo::new())]`) does not work.
- The interceptor is generic over `R` (or constrained to a specific type if needed).

## Complete example

```rust
use std::future::Future;
use r2e_core::prelude::*;

/// Custom interceptor
pub struct AuditLog;

impl<R: Send> Interceptor<R> for AuditLog {
    fn around<F, Fut>(&self, ctx: InterceptorContext, next: F)
        -> impl Future<Output = R> + Send
    where F: FnOnce() -> Fut + Send, Fut: Future<Output = R> + Send,
    {
        async move {
            tracing::info!(method = ctx.method_name, "audit: entering");
            let result = next().await;
            tracing::info!(method = ctx.method_name, "audit: done");
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

    #[identity]
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

    // Transaction
    #[post("/db")]
    #[transactional]
    async fn create_in_db(&self, axum::Json(body): axum::Json<CreateUserRequest>)
        -> Result<axum::Json<User>, r2e_core::HttpError>
    {
        sqlx::query("INSERT INTO users ...").execute(&mut *tx).await?;
        Ok(axum::Json(user))
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
