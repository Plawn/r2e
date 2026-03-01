# API Documentation

Full API documentation is available via `cargo doc`.

## Generating docs

```bash
cargo doc --workspace --no-deps --open
```

This generates and opens documentation for all R2E crates.

## Key types by crate

### r2e-core

| Type | Description |
|------|-------------|
| `AppBuilder` | Fluent application builder |
| `HttpError` | Built-in error type with HTTP status mapping |
| `R2eConfig` | YAML + env var configuration |
| `Controller` | Trait for route registration |
| `Guard<S, I>` | Post-auth guard trait |
| `PreAuthGuard<S>` | Pre-auth guard trait |
| `GuardContext<I>` | Guard context with identity |
| `PreAuthGuardContext` | Guard context without identity |
| `Identity` | Identity trait for guards |
| `Interceptor<R>` | AOP interceptor trait |
| `InterceptorContext` | Interceptor context |
| `ManagedResource<S>` | Managed resource lifecycle trait |
| `ManagedErr<E>` | Error wrapper for managed resources |
| `Plugin<S>` | Post-state plugin trait |
| `PreStatePlugin` | Pre-state plugin trait |
| `DeferredAction` | Deferred setup action for plugins |
| `StatefulConstruct<S>` | Construct from state (no HTTP context) |
| `Bean` | Sync bean trait |
| `AsyncBean` | Async bean trait |
| `Producer` | Factory trait for external types |
| `BeanContext` | Bean graph context |
| `Validate` | Re-export of `garde::Validate` for automatic validation |
| `Params` | Derive macro for aggregating path/query/header params |

### r2e-macros

| Macro | Description |
|-------|-------------|
| `#[derive(Controller)]` | Generate controller metadata and extractor |
| `#[routes]` | Generate Axum handlers and Controller impl |
| `#[bean]` | Generate Bean or AsyncBean impl |
| `#[producer]` | Generate Producer impl from free function |
| `#[derive(Bean)]` | Derive Bean from struct fields |
| `#[derive(BeanState)]` | Derive FromRef for state struct |

### r2e-security

| Type | Description |
|------|-------------|
| `AuthenticatedUser` | JWT identity extractor |
| `JwtClaimsValidator` | Low-level JWT validator |
| `JwtValidator` | High-level JWT validator with identity builder |
| `SecurityConfig` | JWT/JWKS configuration |
| `JwksCache` | JWKS key cache |
| `RoleExtractor` | Trait for custom role extraction |

### r2e-events

| Type | Description |
|------|-------------|
| `EventBus` | Pluggable event bus trait |
| `LocalEventBus` | Default in-process pub/sub implementation |

### r2e-scheduler

| Type | Description |
|------|-------------|
| `Scheduler` | PreStatePlugin for background tasks |
| `ScheduleConfig` | Interval/cron/delay configuration |
| `ScheduledTaskDef<T>` | Task definition |

### r2e-data

| Type | Description |
|------|-------------|
| `Entity` | Table mapping trait |
| `QueryBuilder` | Fluent SQL builder |
| `Pageable` | Pagination parameters |
| `Page<T>` | Paginated response |
| `DataError` | Data layer error |

### r2e-data-sqlx

| Type | Description |
|------|-------------|
| `SqlxRepository<E, DB>` | SQLx-backed repository |
| `Tx<DB>` | Transaction wrapper |
| `HasPool<DB>` | Pool accessor trait |

### r2e-cache

| Type | Description |
|------|-------------|
| `TtlCache<K, V>` | Thread-safe TTL cache |
| `CacheStore` | Pluggable cache backend trait |
| `InMemoryStore` | Default in-memory backend |

### r2e-rate-limit

| Type | Description |
|------|-------------|
| `RateLimiter<K>` | Token-bucket rate limiter |
| `RateLimitRegistry` | Rate limiter handle for app state |
| `RateLimit` | Builder for rate limit guards |
| `RateLimitBackend` | Pluggable backend trait |

### r2e-test

| Type | Description |
|------|-------------|
| `TestApp` | In-process HTTP client |
| `TestResponse` | Response with assertion helpers |
| `TestJwt` | JWT token generator |
