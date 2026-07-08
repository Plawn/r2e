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

`Cache` reads a cache store from the bean graph, so the store must be provided
as a bean (an `Arc<dyn CacheStore>`). A missing store is a **compile error at
`register_controller()`**. Provide one on the builder:

```rust
use r2e::r2e_cache::InMemoryStore;

AppBuilder::new()
    .provide(InMemoryStore::shared())   // Arc<dyn CacheStore>
    // ... other beans ...
    .build_state()
    .await;
```

> There is no global cache store anymore — the old `cache_backend()` /
> `set_cache_backend()` functions have been removed. The store is always a bean.

```rust
#[get("/")]
#[intercept(Cache::ttl(30))]                     // Cache for 30 seconds
async fn list(&self) -> Json<Vec<User>> { /* ... */ }

#[get("/")]
#[intercept(Cache::ttl(30).group("users"))]      // Named cache group
async fn list(&self) -> Json<Vec<User>> { /* ... */ }
```

`Cache` and `CacheInvalidate` are the only built-in interceptors that read a
bean. `Logged`, `Timed`, `Counted`, and `MetricTimed` are self-contained and
need no beans.

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

## Scheduled tasks and gRPC methods

`#[intercept(...)]` also works on `#[scheduled]` methods and on methods in a
`#[grpc_routes]` block, with the same construction model as HTTP routes: the
interceptor is built **once at registration**, from the resolved bean graph,
and wraps every task tick / RPC call. Bean-reading interceptors (e.g. one
declared with `#[derive(DecoratorBean)]` and `#[inject]` fields) work there
too:

```rust
#[routes]
impl ReportJobs {
    #[scheduled(every = 60)]
    #[intercept(DbAuditLog::spec("nightly"))]  // reads beans — built from the graph
    async fn refresh(&self) { /* ... */ }
}
```

Differences from HTTP routes:

- Guards don't apply (there is no request to reject) — only interceptors.
- Type-constrained interceptors must match the method's return type: a
  scheduled method returns `()` or `Result<(), E>`, so `Cache` (which needs a
  `Cacheable` return) doesn't apply there.
- **Async scheduled methods run their interceptors on direct calls too**:
  the chain wraps the method body itself, so calling `self.refresh()` from
  another method (say, an admin route forcing a run) still goes through the
  interceptors. Two caveats: a *sync* scheduled method's chain only runs
  around scheduler ticks (a sync body can't await the chain), and a
  controller built by hand in a test — without going through registration —
  runs its scheduled methods undecorated.

## Execution order

When multiple interceptors are applied, they wrap in this order (outermost to innermost):

1. Controller-level interceptors (declaration order)
2. Method-level interceptors (declaration order)
3. Method body

Interceptors always see the handler's **raw return type** (`Json<T>`, `Result<Json<T>, E>`, etc.). The `IntoResponse` conversion to `Response` happens after the outermost interceptor.

## Combining with guards and roles

`#[intercept(...)]` works alongside `#[roles]`, `#[guard]`, and `#[pre_guard]`. Guards run before the interceptor chain and short-circuit independently — they don't affect the return type that interceptors see:

```rust
#[get("/admin/users")]
#[roles("admin")]                                  // Guard: 403 if not admin
#[intercept(Cache::ttl(30).group("admin_users"))]  // Cache: sees Json<Vec<User>>, not Response
async fn admin_list(&self) -> Json<Vec<User>> { /* ... */ }
```

> **Known limitation:** `#[managed]` parameters combined with type-constrained interceptors (e.g., `Cache`) don't work because the managed resource lifecycle wraps `into_response` inside the interceptor closure, so `Cache` sees `Response` instead of the raw type. Workaround: inject the store bean (`#[inject] store: Arc<dyn CacheStore>`) and cache manually in the handler body.

## Writing custom interceptors

Implement the `Interceptor<R>` trait:

A self-contained interceptor (no bean dependencies) opts in with one line,
`impl SelfBuilt for AuditLog {}`:

```rust
use r2e::prelude::*; // Interceptor, InterceptorContext, SelfBuilt
use std::future::Future;

pub struct AuditLog;

impl SelfBuilt for AuditLog {}

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

The `#[intercept(...)]` attribute's leading type path names the decorator spec.
For a self-contained interceptor, that type is the interceptor itself
(`impl SelfBuilt`). An interceptor that needs beans holds them as `#[inject]`
fields and derives `DecoratorBean` — plain fields become config passed to the
generated `spec(...)` constructor at the attribute site:

```rust
#[derive(DecoratorBean)]
pub struct DbAuditLog {
    #[inject]
    pool: SqlitePool,     // from the bean graph, compile-checked
    prefix: String,       // config, set at the site
}

impl<R: Send> Interceptor<R> for DbAuditLog { /* uses self.pool */ }

#[get("/")]
#[intercept(DbAuditLog::spec("api".into()))]
async fn list(&self) -> Json<Vec<User>> { /* ... */ }
```

See [Custom Guards](./custom-guards.md#guards-that-read-beans) for the full
field-attribute reference (`#[inject]`, `#[config]`, plain fields) and the
low-level `DecoratorSpec` contract the derive expands to.

### `InterceptorContext`

| Field | Type | Description |
|-------|------|-------------|
| `controller_name` | `&'static str` | Controller struct name |
| `method_name` | `&'static str` | Handler method name |

## Performance

Interceptors are monomorphized (no `dyn`, no vtable). The overhead is ~100 ns per interceptor — effectively free in any real application.
