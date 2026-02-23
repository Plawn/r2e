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

Method body level (trait-based, via `Interceptor::around`, in `generate_wrapped_method`):
4. `logged`
5. `timed`
6. User-defined interceptors (`#[intercept(...)]`)
7. `cached`

Inline codegen (no trait):
8. `cache_invalidate` (after body)
9. `transactional` (wraps body in tx begin/commit)
10. Original method body

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
#[guard(MyCustomGuard)]                      // custom post-auth guard (async)
#[pre_guard(MyPreAuthGuard)]                 // custom pre-auth guard (runs before JWT)
#[middleware(my_middleware_fn)]               // Tower middleware
```

**User-defined interceptors** implement `Interceptor<R>` and are applied via `#[intercept(TypeName)]`. The type must be constructable as a bare path expression (unit struct or constant).
