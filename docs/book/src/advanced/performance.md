# Performance Guide

R2E is designed for minimal overhead. Understanding the cost model helps you make informed decisions.

## Per-request cost breakdown

| Operation | Cost | Notes |
|-----------|------|-------|
| Tower layer traversal | ~1 us | TraceLayer, CorsLayer, etc. |
| Axum routing | ~1 us | Path matching |
| `#[inject]` field clone | ~10-50 ns each | O(1) if `Arc` |
| `#[config("key")]` lookup | ~50 ns each | HashMap lookup |
| JWT validation | ~10-50 us | Signature verification |
| JWKS lookup (cache miss) | ~50-200 ms | HTTP roundtrip (rare) |
| Rate limit check | ~100 ns | DashMap lookup |
| Role guard check | ~50 ns | Vec scan |
| Interceptor overhead | ~100 ns each | Monomorphized, no vtable |
| Business logic | variable | DB I/O, external services |

## Optimization guidelines

### 1. Wrap services in `Arc`

Service clones happen per request. With `Arc`, it's a reference count increment (~10 ns) vs a deep copy:

```rust
#[derive(Clone)]
pub struct UserService {
    inner: Arc<UserServiceInner>,
}
```

### 2. Use param-level identity for mixed controllers

Struct-level `#[inject(identity)]` runs JWT validation for **every** endpoint — even public ones. Param-level only validates when needed:

```rust
// Bad: JWT validated for public endpoints too
#[derive(Controller)]
pub struct ApiController {
    #[inject(identity)] user: AuthenticatedUser,  // validates on ALL endpoints
}

// Good: JWT only validated where needed
#[derive(Controller)]
pub struct ApiController {
    #[inject] service: MyService,
}

#[routes]
impl ApiController {
    #[get("/public")]
    async fn public_data(&self) -> Json<Data> { /* no JWT overhead */ }

    #[get("/me")]
    async fn me(&self, #[inject(identity)] user: AuthenticatedUser) -> Json<User> {
        Json(user)
    }
}
```

### 3. Minimize `#[config]` fields

Each `#[config]` field performs a HashMap lookup per request. For frequently accessed config, consider caching the value in a service field at startup.

### 4. Pre-warm JWKS cache

The first JWT validation with a JWKS endpoint incurs a ~50-200 ms HTTP roundtrip. Pre-warm in an `on_start` hook:

```rust
.on_start(|state| async move {
    // Trigger a JWKS cache refresh
    state.validator.refresh_jwks().await?;
    Ok(())
})
```

### 5. Interceptors are free

Interceptors use monomorphization (no `dyn`, no vtable). The overhead is ~100 ns — effectively free. Don't hesitate to use `Logged`, `Timed`, etc.

### 6. Guards should be O(1)

Guards run on every guarded request. Keep them fast:
- Role checks: O(n) where n = number of required roles (typically small)
- Rate limit checks: O(1) DashMap lookup
- Avoid database queries in guards when possible

### 7. One controller per responsibility

Avoid putting unrelated endpoints in the same controller. Each controller shares the same injection profile — injecting unnecessary services adds clone overhead.

## Anti-patterns

### Storing `R2eConfig` as `#[inject]`

```rust
// Bad: clones the entire config HashMap per request
#[inject] config: R2eConfig,

// Good: use specific #[config] fields
#[config("app.name")] name: String,
```

### I/O in custom guards

```rust
// Bad: blocks the Tokio runtime
impl<S, I: Identity> Guard<S, I> for SlowGuard {
    fn check(&self, state: &S, ctx: &GuardContext<'_, I>) -> impl Future<...> + Send {
        async move {
            // This is a database query on EVERY request
            let result = sqlx::query("SELECT ...").fetch_one(&pool).await;
            // ...
        }
    }
}
```

If you must do I/O in guards, consider caching results or moving the check to the handler body.

## Comparison: struct-level vs param-level identity

| Aspect | Struct-level | Param-level |
|--------|-------------|------------|
| JWT validation | Every request | Only annotated endpoints |
| `StatefulConstruct` | Not generated | Generated |
| Consumers/Schedulers | Not possible | Possible |
| Identity access | `self.user` | Only in handler parameter |
| Public endpoint overhead | JWT validation wasted | None |
