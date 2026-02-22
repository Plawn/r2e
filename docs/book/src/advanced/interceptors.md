# Interceptors

Interceptors implement cross-cutting concerns (logging, timing, caching) via a generic `Interceptor<R>` trait with an `around` pattern. All calls are monomorphized — zero runtime overhead.

## Built-in interceptors

R2E provides four built-in interceptors in `r2e-utils`:

### `Logged` — Request logging

```rust
#[routes]
#[intercept(Logged::info())]  // Log all methods at INFO level
impl UserController {
    #[get("/")]
    async fn list(&self) -> Json<Vec<User>> { /* ... */ }
}
```

Available levels: `Logged::trace()`, `Logged::debug()`, `Logged::info()`, `Logged::warn()`, `Logged::error()`.

### `Timed` — Execution timing

```rust
#[get("/")]
#[intercept(Timed::new())]              // Always log execution time
async fn list(&self) -> Json<Vec<User>> { /* ... */ }

#[get("/slow")]
#[intercept(Timed::threshold(50))]      // Only log if >50ms
async fn slow_query(&self) -> Json<Vec<User>> { /* ... */ }
```

### `Cache` — Response caching

```rust
#[get("/")]
#[intercept(Cache::ttl(30))]                     // Cache for 30 seconds
async fn list(&self) -> Json<Vec<User>> { /* ... */ }

#[get("/")]
#[intercept(Cache::ttl(30).group("users"))]      // Named cache group
async fn list(&self) -> Json<Vec<User>> { /* ... */ }
```

### `CacheInvalidate` — Clear cache groups

```rust
#[post("/")]
#[intercept(CacheInvalidate::group("users"))]    // Clear "users" cache after execution
async fn create(&self, body: Json<Request>) -> Json<User> { /* ... */ }
```

## Controller-level interceptors

Apply to all methods in a controller:

```rust
#[routes]
#[intercept(Logged::info())]
impl UserController {
    // All methods get logged
}
```

## Method-level interceptors

Apply to individual methods:

```rust
#[routes]
impl UserController {
    #[get("/")]
    #[intercept(Timed::threshold(100))]
    #[intercept(Cache::ttl(60).group("users"))]
    async fn list(&self) -> Json<Vec<User>> { /* ... */ }

    #[post("/")]
    #[intercept(CacheInvalidate::group("users"))]
    async fn create(&self, body: Json<Request>) -> Json<User> { /* ... */ }
}
```

## Execution order

When multiple interceptors are applied, they wrap in this order (outermost to innermost):

1. `Logged` (controller-level, then method-level)
2. `Timed`
3. User-defined interceptors (`#[intercept(...)]`)
4. `Cache`
5. Method body
6. `CacheInvalidate` (after body)

## Writing custom interceptors

Implement the `Interceptor<R>` trait:

```rust
use r2e::prelude::*; // Interceptor, InterceptorContext
use std::future::Future;

pub struct AuditLog;

impl<R: Send> Interceptor<R> for AuditLog {
    fn around<F, Fut>(&self, ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        async move {
            tracing::info!(
                controller = ctx.controller_name,
                method = ctx.method_name,
                "audit: entering"
            );
            let result = next().await;
            tracing::info!(
                controller = ctx.controller_name,
                method = ctx.method_name,
                "audit: completed"
            );
            result
        }
    }
}
```

Apply it:

```rust
#[get("/")]
#[intercept(AuditLog)]
async fn list(&self) -> Json<Vec<User>> { /* ... */ }
```

The type must be constructable as a bare path expression (unit struct or constant).

### `InterceptorContext`

| Field | Type | Description |
|-------|------|-------------|
| `controller_name` | `&'static str` | Controller struct name |
| `method_name` | `&'static str` | Handler method name |

## Performance

Interceptors are monomorphized (no `dyn`, no vtable). The overhead is ~100 ns per interceptor — effectively free in any real application.
