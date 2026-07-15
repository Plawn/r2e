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
| `Guard<I>` | Post-auth guard trait (built once from the bean graph) |
| `PreAuthGuard` | Pre-auth guard trait |
| `GuardContext<I>` | Guard context with identity |
| `PreAuthGuardContext` | Guard context without identity |
| `Identity` | Identity trait for guards |
| `Interceptor<R>` | AOP interceptor trait, generic over the return type `R` |
| `InterceptorContext` | Interceptor context (`Copy`: method + controller name) |
| `DecoratorSpec` | Build contract for guards/interceptors (Product + Deps + build) |
| `SelfBuilt` | Marker for self-contained decorators (no bean deps) |
| `DecoratorBean` (derive) | Generates the `DecoratorSpec` plumbing for a bean-reading guard/interceptor (`#[inject]` fields + `Type::spec(...)` constructor) |
| `ManagedResource<S>` | Managed resource lifecycle trait |
| `ManagedErr<E>` | Error wrapper for managed resources |
| `Plugin<S>` | Post-state plugin trait |
| `PreStatePlugin` | Pre-state plugin trait |
| `DeferredAction` | Deferred setup action for plugins |
| `ContextConstruct` | Construct a controller from the resolved bean graph by type (no HTTP context) — replaces the removed `StatefulConstruct` |
| `Bean` | Sync bean trait |
| `AsyncBean` | Async bean trait |
| `Producer` | Factory trait for external types |
| `BeanContext` | Resolved bean graph; beans are fetched by type (`ctx.get::<T>()`) |
| `BeanLookup` | Dynamic, witness-free bean access on the state (`state.bean::<T>() -> Option<T>`); the vocabulary for guards, interceptors, and `ManagedResource`. In the prelude |
| `BeanAccess` | Witness-free fixed-offset bean access on the state (`state.get::<T>()`). Not in the prelude — import via `use r2e_core::type_list::BeanAccess;` |
| `FromRequestPartsVia<S, M>` | Request-scoped extractor trait (identity + `#[inject(request)]`); `OptionalFromRequestPartsVia<S, M>` is the optional variant. Plain axum `FromRequestParts` extractors bridge automatically via `ViaAxum` |
| `Validate` | Re-export of `garde::Validate` for automatic validation |
| `Params` | Derive macro for aggregating path/query/header params |

### r2e-macros

| Macro | Description |
|-------|-------------|
| `#[controller]` | Declare a controller: generate metadata, request façade, and routes |
| `#[routes]` | Generate Axum handlers and Controller impl |
| `#[bean]` | Generate Bean or AsyncBean impl |
| `#[producer]` | Generate Producer impl from free function |
| `#[derive(Bean)]` | Derive Bean from struct fields |

> **State is inferred, not declared.** There is no state struct and no state-deriving
> macro. Beans are `.provide()`-d or `.register::<T>()`-ed on the builder, and
> `.build_state().await` materializes the application state as an HList inferred from
> the provision list. `#[controller]` resolves its `#[inject]` fields from that graph
> **by type** — a missing bean is a compile error naming the type. (The old
> `#[derive(BeanState)]` / `BeanState` trait and `build_state!` macro have been removed.)

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

### r2e-core pagination

| Type | Description |
|------|-------------|
| `Pageable` | Pagination parameters |
| `Page<T>` | Paginated response |

### r2e-data-sqlx

| Type | Description |
|------|-------------|
| `Tx<'a, DB>` / `SqlxTx<'a, DB>` | Cancellation-safe managed SQLx transaction |

### r2e-data-diesel

| Type | Description |
|------|-------------|
| `Tx<C>` / `DieselTx<C>` | Managed Diesel transaction on an r2d2 connection |

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
