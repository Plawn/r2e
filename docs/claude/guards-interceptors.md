# Guards & Interceptors

## Guards

Handler-level guards run before controller construction and can short-circuit with an error response. The `Guard<S, I: Identity>` trait (`r2e-core/src/guards.rs`) defines an async `check(&self, state, ctx) -> impl Future<Output = Result<(), Response>> + Send` method. Guards are generic over both the application state `S` and the identity type `I`.

`GuardContext<'a, I: Identity>` provides:
- `method_name`, `controller_name` — handler identification
- `headers` — request headers (`&HeaderMap`)
- `uri` — request URI (`&Uri`) with convenience methods `path()` and `query_string()`
- `identity` — optional identity reference (`Option<&'a I>`)
- Convenience accessors: `identity_sub()`, `identity_roles()`, `identity_email()`, `identity_claims()`

The `Identity` trait (`r2e-core::Identity`) decouples guards from the concrete `AuthenticatedUser` type:
- `sub()` — unique subject identifier (required)
- `roles()` — role list (required)
- `email()` — email address (optional, default `None`)
- `claims()` — raw JWT claims as `serde_json::Value` (optional, default `None`)

`NoIdentity` is a sentinel type used when no identity is available.

### Built-in guards

- `RolesGuard` — checks required roles, returns 403 if missing. Applied via `#[roles("admin")]`. Implements `Guard<S, I>` for any `I: Identity`.
- `RateLimitGuard` / `PreAuthRateLimitGuard` — token-bucket rate limiting, returns 429. Use the `RateLimit` builder with `#[guard(...)]` or `#[pre_guard(...)]`:
  ```rust
  use r2e::r2e_rate_limit::RateLimit;

  #[pre_guard(RateLimit::global(5, 60))]    // 5 req / 60 sec, shared bucket (pre-auth)
  #[pre_guard(RateLimit::per_ip(5, 60))]    // 5 req / 60 sec, per IP (pre-auth)
  #[guard(RateLimit::per_user(5, 60))]      // 5 req / 60 sec, per user (post-auth)
  ```

### Pre-authentication guards

For authorization checks that don't require identity (e.g., IP-based rate limiting, allowlisting), use the `PreAuthGuard<S>` trait. Pre-auth guards run as middleware **before** JWT extraction, avoiding wasted token validation when requests will be rejected.

- `PreAuthGuardContext` — provides `method_name`, `controller_name`, `headers`, `uri` (no identity)
- `PreAuthRateLimitGuard` — pre-auth rate limiter for global/IP keys
- Apply custom pre-auth guards via `#[pre_guard(MyPreAuthGuard)]`

### Rate-limiting key classification

- `RateLimit::global()` / `RateLimit::per_ip()` → use with `#[pre_guard(...)]` (runs before JWT validation)
- `RateLimit::per_user()` → use with `#[guard(...)]` (runs after JWT validation, needs identity)

### Custom guards

- Post-auth: implement `Guard<S, I: Identity>` (async via RPITIT) and apply via `#[guard(MyGuard)]`
- Pre-auth: implement `PreAuthGuard<S>` and apply via `#[pre_guard(MyPreAuthGuard)]`

**Async guard:** implement `Guard<S, I: Identity>` with async `check()` that returns `impl Future<Output = Result<(), Response>> + Send`. Can use `FromRef<S>` to access state (e.g., database pools).

## Interceptors

Cross-cutting concerns (logging, timing, caching) are implemented via a generic `Interceptor<R>` trait with an `around` pattern (`r2e-core/src/interceptors.rs`). All calls are monomorphized (no `dyn`) for zero overhead.

### Built-in interceptors (in `r2e-utils`)

- `Logged` — logs entry/exit at a configurable `LogLevel`.
- `Timed` — measures execution time, with an optional threshold (only logs if exceeded).
- `Counted` — increments a named counter on each invocation, logged via `tracing`. Builder: `Counted::new("metric_name")`, optionally `.with_level(LogLevel)`.
- `MetricTimed` — records execution duration as a named metric, logged via `tracing`. Builder: `MetricTimed::new("metric_name")`, optionally `.with_level(LogLevel)`. Unlike `Timed`, always logs with the metric name (designed for metrics collection rather than debugging).
- `Cache` — caches `Json<T>` responses via the global `CacheStore`. Supports TTL and named groups.
- `CacheInvalidate` — clears a named cache group after method execution.

### Interceptor wrapping order (outermost → innermost)

Pre-auth middleware level (runs BEFORE Axum extraction/JWT validation):
0. `pre_guard(RateLimit::global(...))` / `pre_guard(RateLimit::per_ip(...))` — pre-auth rate limiting
0. `pre_guard(CustomPreAuthGuard)` — custom pre-auth guards

Handler level (after extraction, before controller body):
1. `guard(RateLimit::per_user(...))` — per-user rate limiting (needs identity)
2. `roles` — short-circuits with 403
3. `guard(CustomGuard)` — custom guards, short-circuit with custom error

Method body level (trait-based, via `Interceptor::around`):
4. Controller-level interceptors (declaration order)
5. Method-level interceptors (declaration order)

Inline codegen (no trait):
6. `transactional` (wraps body in tx begin/commit)
7. Original method body

**Design invariant:** Interceptors always see the handler's **raw return type** (`Json<T>`, `Result<Json<T>, E>`, etc.), never `Response`. The `IntoResponse::into_response()` conversion happens *after* the outermost interceptor. Guards short-circuit *before* interceptors, so they don't affect the type interceptors see.

### `Cache` interceptor type constraints

`Cache` requires `R: Cacheable`. Built-in `Cacheable` impls:
- `Json<T>` where `T: Serialize + DeserializeOwned + Send`
- `Result<T, E>` where `T: Cacheable, E: Send` (only caches `Ok` values)
- Types deriving `#[derive(Cacheable)]`

Other built-in interceptors (`Logged`, `Timed`, `CacheInvalidate`, `Counted`, `MetricTimed`) only require `R: Send` and work with any return type.

```rust
#[intercept(Counted::new("user_list_total"))]           // count invocations
#[intercept(MetricTimed::new("user_list_duration"))]    // record duration as named metric
async fn list(&self) -> Json<Vec<User>> { /* ... */ }
```

### Combining interceptors with guards/roles

`#[intercept(Cache)]` + `#[roles]` (or any `#[guard]`) works correctly — guards run first, then interceptors see the raw type:
```rust
#[get("/admin/users")]
#[roles("admin")]
#[intercept(Cache::ttl(30).group("admin_users"))]
async fn admin_list(&self) -> Json<Vec<User>> { /* ... */ }
```

**Known limitation:** `#[managed]` + `#[intercept(Cache)]` does NOT work — the managed resource lifecycle (acquire/release with error handling) wraps `into_response` inside the interceptor closure, so `Cache` sees `Response` instead of the raw type. Workaround: use `cache_backend()` manually in the handler body.

### Configurable syntax

```rust
#[transactional]                             // uses self.pool
#[transactional(pool = "read_db")]           // custom pool field
#[pre_guard(RateLimit::global(5, 60))]       // global rate limit (pre-auth)
#[pre_guard(RateLimit::per_ip(5, 60))]       // per-IP rate limit (pre-auth)
#[guard(RateLimit::per_user(5, 60))]         // per-user rate limit (post-auth, requires identity)
#[intercept(MyInterceptor)]                  // user-defined (must be a unit struct/constant)
#[intercept(Logged::info())]                 // built-in interceptor with config
#[intercept(Cache::ttl(30).group("users"))]  // cache with named group
#[intercept(CacheInvalidate::group("users"))] // invalidate cache group
#[intercept(Counted::new("metric_name"))]    // count invocations
#[intercept(MetricTimed::new("metric_name"))] // record duration as named metric
#[guard(MyCustomGuard)]                      // custom post-auth guard (async)
#[pre_guard(MyPreAuthGuard)]                 // custom pre-auth guard (runs before JWT)
#[middleware(my_middleware_fn)]               // Tower middleware via from_fn
#[layer(TimeoutLayer::new(Duration::from_secs(5)))] // arbitrary Tower Layer
#[status(200)]                               // override OpenAPI status code
#[returns(MyType)]                           // explicit OpenAPI response type
#[raw]                                       // marker for raw Axum extractors (no-op)
```

**User-defined interceptors** implement `Interceptor<R>` and are applied via `#[intercept(TypeName)]`. The type must be constructable as a bare path expression (unit struct or constant).

## Tower Middleware & Layers

### `#[middleware]` — Tower middleware functions

Applies a Tower middleware function to a specific route via `axum::middleware::from_fn`. The function must follow Axum's middleware signature: `async fn(Request, Next) -> Response`.

```rust
#[get("/data")]
#[middleware(require_api_key)]
async fn protected_data(&self) -> Json<Vec<Item>> { /* ... */ }
```

Multiple `#[middleware]` attributes can be stacked; they apply outermost-first in declaration order:
```rust
#[get("/data")]
#[middleware(log_request)]       // runs first
#[middleware(require_api_key)]   // runs second
async fn protected_data(&self) -> Json<Vec<Item>> { /* ... */ }
```

Generated code calls `.layer(axum::middleware::from_fn(name))` on the route handler.

### `#[layer]` — Arbitrary Tower layers

Accepts any expression evaluating to a Tower `Layer`. Use for `tower-http` and other Tower-compatible layers directly.

```rust
use tower_http::timeout::TimeoutLayer;

#[get("/slow")]
#[layer(TimeoutLayer::new(Duration::from_secs(5)))]
async fn slow_operation(&self) -> Json<&'static str> { /* ... */ }
```

Can be combined with `#[middleware]` on the same route — both emit `.layer()` calls on the handler.

Common layers: `TimeoutLayer` (tower-http), `SetResponseHeaderLayer` (tower-http), `CorsLayer` (tower-http), `CompressionLayer` (tower-http), `ConcurrencyLimitLayer` (tower).

## Route Annotation Attributes

### `#[status(CODE)]` — Override HTTP status code for OpenAPI

Overrides the default success status code in the generated OpenAPI spec. Does not change the actual HTTP response — that is determined by the handler's return type and `IntoResponse` impl.

Defaults: GET->200, POST->201, PUT->200, PATCH->200, DELETE->204.

```rust
#[post("/users/search")]
#[status(200)]                  // POST normally defaults to 201
async fn search(&self, body: Json<Query>) -> JsonResult<Vec<User>> { /* ... */ }

#[post("/jobs")]
#[status(202)]                  // 202 Accepted for async processing
async fn submit_job(&self, body: Json<JobRequest>) -> JsonResult<JobId> { /* ... */ }
```

### `#[returns(Type)]` — Explicit response type for OpenAPI

Declares the response body type when the macro cannot infer it (e.g., `impl IntoResponse`, custom wrappers). The OpenAPI spec will use the given type's schema.

```rust
#[get("/widgets/{id}")]
#[returns(Widget)]
async fn get_widget(&self, Path(id): Path<u64>) -> impl IntoResponse {
    Json(self.service.find(id).await)
}
```

Combines well with `#[status]` for full OpenAPI control:
```rust
#[post("/orders")]
#[status(202)]
#[returns(OrderReceipt)]
async fn place_order(&self, body: Json<OrderRequest>) -> impl IntoResponse { /* ... */ }
```

### `#[raw]` — Mark raw Axum extractors

Documentation-only marker with no effect on code generation. Signals that a handler parameter is a raw Axum extractor passed through as-is, not an R2E-managed type. Useful for clarity when using uncommon extractors alongside `#[inject(identity)]` or `#[managed]` parameters.

```rust
#[post("/upload")]
async fn upload(
    &self,
    #[inject(identity)] user: AuthenticatedUser,
    #[raw] multipart: Multipart,       // explicit: this is a raw Axum extractor
) -> Result<Json<UploadResult>, HttpError> { /* ... */ }
```
