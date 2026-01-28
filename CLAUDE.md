# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

```bash
# Build all crates
cargo build --workspace

# Check all crates (faster, no codegen)
cargo check --workspace

# Run the example application (serves on 0.0.0.0:3000)
cargo run -p example-app

# Run tests
cargo test --workspace

# Build a specific crate
cargo build -p quarlus-core
cargo build -p quarlus-macros
cargo build -p quarlus-security
cargo build -p quarlus-events
cargo build -p quarlus-scheduler
cargo build -p quarlus-data
cargo build -p quarlus-cache
cargo build -p quarlus-rate-limit
cargo build -p quarlus-openapi
cargo build -p quarlus-utils
cargo build -p quarlus-test
cargo build -p quarlus-cli

# Expand macros for debugging (requires cargo-expand)
cargo expand -p example-app
```

## Architecture

Quarlus is a **Quarkus-like ergonomic layer over Axum** for Rust. It provides declarative controllers with compile-time dependency injection, JWT/OIDC security, and zero runtime reflection.

### Workspace Crates

```
quarlus-macros      → Proc-macro crate (no runtime deps). #[derive(Controller)] + #[routes] generate Axum handlers.
quarlus-core        → Runtime foundation. AppBuilder, Controller trait, StatefulConstruct trait, AppError, Guard trait,
                      Interceptor trait, QuarlusConfig, lifecycle hooks, Tower layers, dev-mode endpoints.
quarlus-security    → JWT validation, JWKS cache, AuthenticatedUser extractor, RoleExtractor trait.
quarlus-events      → In-process EventBus with typed pub/sub (emit, emit_and_wait, subscribe).
quarlus-scheduler   → Background task scheduling (interval, cron, initial delay). CancellationToken-based shutdown.
quarlus-data        → Data access: Entity trait, QueryBuilder, Repository trait, SqlxRepository, Pageable/Page.
quarlus-cache       → TtlCache, pluggable CacheStore trait (default InMemoryStore), global cache backend singleton.
quarlus-rate-limit  → Token-bucket RateLimiter, pluggable RateLimitBackend trait, RateLimitRegistry, RateLimitGuard.
quarlus-openapi     → OpenAPI 3.0.3 spec generation from route metadata, Swagger UI at /docs.
quarlus-utils       → Built-in interceptors: Logged, Timed, Cache, CacheInvalidate.
quarlus-test        → Test helpers: TestApp (HTTP client wrapper), TestJwt (JWT generation for tests).
quarlus-cli         → CLI tool: quarlus new, quarlus add, quarlus dev, quarlus generate.
example-app         → Demo binary exercising all features.
```

Dependency flow: `quarlus-macros` ← `quarlus-core` ← `quarlus-security` / `quarlus-events` / `quarlus-scheduler` / `quarlus-data` / `quarlus-cache` / `quarlus-rate-limit` / `quarlus-openapi` / `quarlus-utils` / `quarlus-test` ← `example-app`

### Core Concepts

**Three injection scopes, all resolved at compile time:**
- `#[inject]` — App-scoped. Field is cloned from the Axum state (services, repos, pools). Type must be `Clone + Send + Sync`.
- `#[inject(identity)]` — Request-scoped. Field is extracted via Axum's `FromRequestParts` (e.g., `AuthenticatedUser` from JWT). Type must implement `Identity`. Legacy `#[identity]` syntax is still supported.
- `#[config("key")]` — App-scoped. Field is resolved from `QuarlusConfig` at request time. Field type must implement `FromConfigValue` (`String`, `i64`, `f64`, `bool`, `Option<T>`).

**Handler parameter-level identity injection:**
- `#[inject(identity)]` can also be placed on handler parameters (not just struct fields). This enables mixed controllers where some endpoints are public and others require authentication, while still allowing `StatefulConstruct` generation for consumers and scheduled tasks.

**Controller declaration uses two macros working together:**

1. `#[derive(Controller)]` on the struct — generates metadata module, Axum extractor, and `StatefulConstruct` impl (when no identity fields).
2. `#[routes]` on the impl block — generates Axum handler functions, `Controller<T>` trait impl, and `ScheduledController<T>` impl.

```rust
// Struct-level identity (all endpoints require auth)
#[derive(Controller)]
#[controller(path = "/users", state = Services)]
pub struct UserController {
    #[inject]  user_service: UserService,
    #[inject(identity)] user: AuthenticatedUser,
    #[config("app.greeting")] greeting: String,
}

#[routes]
#[intercept(Logged::info())]
impl UserController {
    #[get("/")]
    async fn list(&self) -> Json<Vec<User>> {
        Json(self.user_service.list().await)
    }
}

// Mixed controller (param-level identity — public + protected endpoints)
#[derive(Controller)]
#[controller(path = "/api", state = Services)]
pub struct MixedController {
    #[inject] user_service: UserService,
}

#[routes]
impl MixedController {
    #[get("/public")]
    async fn public_data(&self) -> Json<Vec<Data>> { ... }

    #[get("/me")]
    async fn me(&self, #[inject(identity)] user: AuthenticatedUser) -> Json<AuthenticatedUser> {
        Json(user)
    }
}
```

**Generated items (hidden):**
- `mod __quarlus_meta_<Name>` — contains `type State`, `type IdentityType`, `const PATH_PREFIX`, `fn guard_identity()`
- `struct __QuarlusExtract_<Name>` — `FromRequestParts` extractor that constructs the controller from state + request parts
- `impl StatefulConstruct<State> for Name` — only when no `#[inject(identity)]` struct fields; used by consumers and scheduled tasks
- Free-standing Axum handler functions (named `__quarlus_<Name>_<method>`)
- `impl Controller<State> for Name` — wires routes into `axum::Router<State>`

### Macro Crate Internals (quarlus-macros)

The proc-macro pipeline has two entry points:

**Derive path:** `lib.rs` → `derive_controller.rs` → `derive_parsing.rs` (DeriveInput → `ControllerStructDef`) → `derive_codegen.rs` (generate meta module, extractor, StatefulConstruct)

**Routes path:** `lib.rs` → `routes_attr.rs` → `routes_parsing.rs` (ItemImpl → `RoutesImplDef`) → `routes_codegen.rs` (generate impl block, handlers, Controller trait impl, ScheduledController impl)

**Shared modules:**
- `types.rs` — shared types (`InjectedField`, `IdentityField`, `ConfigField`, `RouteMethod`, `ConsumerMethod`, `ScheduledMethod`, etc.)
- `attr_extract.rs` — attribute extraction functions (`extract_route_attr`, `extract_roles`, `extract_transactional`, `extract_intercept_fns`, etc.)
- `route.rs` — `HttpMethod` enum and `RoutePath` parser

**Inter-macro liaison:** The derive generates a hidden module `__quarlus_meta_<Name>` and an extractor struct `__QuarlusExtract_<Name>`. The `#[routes]` macro references these by naming convention.

Handler generation pattern: each `#[get("/path")]` method becomes a standalone async function that takes `__QuarlusExtract_<Name>` (which implements `FromRequestParts`) and method parameters. The extractor constructs the controller from state + request parts. For guarded handlers, `State(state)` and `HeaderMap` are also extracted.

**No-op attribute macros:** `lib.rs` declares attributes like `#[get]`, `#[roles]`, `#[intercept]`, `#[guard]`, `#[consumer]`, `#[scheduled]`, `#[middleware]`, etc. as no-op `#[proc_macro_attribute]` that return their input unchanged. These are parsed from the token stream by `#[routes]`. The no-op declarations exist for: (1) preventing "cannot find attribute" errors outside `#[routes]`, (2) `cargo doc` visibility, (3) IDE autocomplete support. The `#[inject]`, `#[identity]`, and `#[config]` attributes are derive helper attributes (consumed by `#[derive(Controller)]`). Note: `#[inject(identity)]` on handler parameters is parsed and stripped by `#[routes]` macro processing.

### Guards

Handler-level guards run before controller construction and can short-circuit with an error response. The `Guard<S, I: Identity>` trait (`quarlus-core/src/guards.rs`) defines a `check(&self, state, ctx) -> Result<(), Response>` method. Guards are generic over both the application state `S` and the identity type `I`.

`GuardContext<'a, I: Identity>` provides: method name, controller name, headers, and an `Option<&I>` for the full identity object. Convenience methods `identity_sub()` and `identity_roles()` access common fields via the `Identity` trait.

The `Identity` trait (`quarlus-core::Identity`) decouples guards from the concrete `AuthenticatedUser` type. It provides `sub()` and `roles()`. `NoIdentity` is a sentinel type used when no identity is available.

**Built-in guards:**
- `RolesGuard` — checks required roles, returns 403 if missing. Applied via `#[roles("admin")]`. Implements `Guard<S, I>` for any `I: Identity`.
- `RateLimitGuard` — token-bucket rate limiting, returns 429. Applied via `#[rate_limited(max = 5, window = 60)]`. Implements `Guard<S, I>` for any `I: Identity`.

**Custom guards:** implement `Guard<S, I: Identity>` (generic) or `Guard<S, AuthenticatedUser>` (specific) and apply via `#[guard(MyGuard)]`.

### Interceptors

Cross-cutting concerns (logging, timing, caching) are implemented via a generic `Interceptor<R>` trait with an `around` pattern (`quarlus-core/src/interceptors.rs`). All calls are monomorphized (no `dyn`) for zero overhead.

**Built-in interceptors** (in `quarlus-utils`):
- `Logged` — logs entry/exit at a configurable `LogLevel`.
- `Timed` — measures execution time, with an optional threshold (only logs if exceeded).
- `Cache` — caches `Json<T>` responses via the global `CacheStore`. Supports TTL and named groups.
- `CacheInvalidate` — clears a named cache group after method execution.

**Interceptor wrapping order** (outermost → innermost):

Handler level (before the controller, in `generate_single_handler`):
1. `rate_limited` — short-circuits with 429
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

**Configurable syntax:**
```rust
#[transactional]                             // uses self.pool
#[transactional(pool = "read_db")]           // custom pool field
#[rate_limited(max = 5, window = 60)]                  // global key
#[rate_limited(max = 5, window = 60, key = "user")]    // per-user (requires identity)
#[rate_limited(max = 5, window = 60, key = "ip")]      // per-IP (X-Forwarded-For)
#[intercept(MyInterceptor)]                  // user-defined (must be a unit struct/constant)
#[intercept(Logged::info())]                 // built-in interceptor with config
#[intercept(Cache::ttl(30).group("users"))]  // cache with named group
#[intercept(CacheInvalidate::group("users"))] // invalidate cache group
#[guard(MyCustomGuard)]                      // custom guard
#[middleware(my_middleware_fn)]               // Tower middleware
```

**User-defined interceptors** implement `Interceptor<R>` and are applied via `#[intercept(TypeName)]`. The type must be constructable as a bare path expression (unit struct or constant).

### Cache (quarlus-cache)

`TtlCache<K, V>` — thread-safe TTL cache backed by `DashMap`. Supports get, insert, remove, clear, evict_expired.

`CacheStore` trait — pluggable async cache backend. Default: `InMemoryStore` (DashMap-backed). Supports get, set, remove, clear, remove_by_prefix. Global singleton via `set_cache_backend()` / `cache_backend()`.

The `Cache` interceptor (in `quarlus-utils`) uses the global `CacheStore` backend. `#[intercept(Cache::ttl(30).group("users"))]` stores in a named group; `#[intercept(CacheInvalidate::group("users"))]` clears by prefix.

### Rate Limiting (quarlus-rate-limit)

`RateLimiter<K>` — generic token-bucket rate limiter keyed by arbitrary type. `RateLimitBackend` trait for pluggable backends (default: `InMemoryRateLimiter`). `RateLimitRegistry` — clonable handle stored in app state, used by the generated `RateLimitGuard`.

Key kinds: `"global"` (shared bucket), `"user"` (per authenticated user sub), `"ip"` (per X-Forwarded-For).

### Security (quarlus-security)

- `AuthenticatedUser` implements `FromRequestParts` and `Identity` — extracts Bearer token, validates via `JwtValidator`, returns user with sub/email/roles/claims.
- `JwtValidator` supports both static keys (testing) and JWKS endpoint (production) via `JwksCache`.
- `SecurityConfig` — configuration for JWT validation (issuer, audience, JWKS URL, static keys).
- `#[roles("admin")]` attribute generates a guard that checks identity roles via the `Identity` trait and returns 403 if missing.
- Role extraction is trait-based (`RoleExtractor`) to support multiple OIDC providers; default (`DefaultRoleExtractor`) checks top-level `roles` and Keycloak's `realm_access.roles`.

### Events (quarlus-events)

`EventBus` — in-process typed pub/sub. Events are dispatched by `TypeId`. Subscribers receive `Arc<E>`.

- `bus.subscribe(|event: Arc<MyEvent>| async { ... })` — register a handler.
- `bus.emit(event)` — fire-and-forget (spawns handlers as concurrent tasks).
- `bus.emit_and_wait(event)` — waits for all handlers to complete.

**Declarative consumers** via `#[consumer(bus = "field_name")]` in a `#[routes]` impl block. The controller must not have `#[inject(identity)]` struct fields (requires `StatefulConstruct`). Consumers are registered automatically by `AppBuilder::register_controller`.

### Scheduling (quarlus-scheduler)

`Scheduler<T>` manages background tasks. Tasks run until a `CancellationToken` is cancelled (graceful shutdown).

**Schedule types:**
- `Schedule::Every(duration)` — fixed interval.
- `Schedule::EveryDelay { interval, initial_delay }` — with initial delay.
- `Schedule::Cron(expr)` — cron expression (via `cron` crate).

**Declarative scheduling** via `#[scheduled]` attribute:
```rust
#[scheduled(every = 30)]                    // every 30 seconds
#[scheduled(every = 60, delay = 10)]        // first run after 10s
#[scheduled(cron = "0 */5 * * * *")]        // cron expression
```

`ScheduledController<T>` trait is auto-generated by `#[routes]`. Register via `AppBuilderSchedulerExt::with_scheduler`:
```rust
AppBuilder::new()
    .with_scheduler(|s| { s.register::<ScheduledJobs>(); })
```

Controllers with `#[inject(identity)]` struct fields cannot be used for scheduling (no `StatefulConstruct` impl). Controllers using param-level `#[inject(identity)]` only retain `StatefulConstruct` and can be used for scheduling.

### Data (quarlus-data)

- `Entity` trait — maps a Rust struct to a SQL table (table name, column list).
- `QueryBuilder` — fluent SQL query builder (`where_eq`, `where_like`, `order_by`, `limit`, `offset`).
- `Repository` trait — async CRUD interface (`find_by_id`, `find_all`, `create`, `update`, `delete`).
- `SqlxRepository` — SQLx-backed implementation of `Repository`.
- `Pageable` — pagination parameters extracted from query string (page, size, sort).
- `Page<T>` — paginated response wrapper (content, total_elements, total_pages, page, size).
- `DataError` — data-layer error type.

### OpenAPI (quarlus-openapi)

- `OpenApiConfig` — configuration for the generated spec (title, version, description). `with_docs_ui(true)` enables the interactive documentation page.
- `AppBuilderOpenApiExt::with_openapi(config)` — registers OpenAPI routes.
- `SchemaRegistry` / `SchemaProvider` — JSON Schema collection for request/response types.
- Route metadata is collected from `Controller::route_metadata()` during `register_controller`.
- Always serves the spec at `/openapi.json`. When `docs_ui` is enabled, also serves an interactive API documentation UI at `/docs`.

### StatefulConstruct (quarlus-core)

`StatefulConstruct<S>` trait allows constructing a controller from state alone (no HTTP context). Auto-generated by `#[derive(Controller)]` when the struct has no `#[inject(identity)]` fields. Used by:
- Consumer methods (`#[consumer]`) — event handlers that run outside HTTP requests
- Scheduled methods (`#[scheduled]`) — background tasks

Controllers with `#[inject(identity)]` struct fields do NOT get this impl. Attempting to use them in consumer/scheduled context produces a compile error with a diagnostic message via `#[diagnostic::on_unimplemented]`. Controllers using param-level `#[inject(identity)]` only retain `StatefulConstruct` — this is the key advantage of the mixed controller pattern.

### AppBuilder (quarlus-core)

Fluent API for assembling a Quarlus application:

```rust
AppBuilder::new()
    .with_state(services)
    .with_config(config)
    .with_cors()                           // or .with_cors_config(custom_layer)
    .with_tracing()
    .with_health()                         // /health → 200 "OK"
    .with_error_handling()                 // catch panics → JSON 500
    .with_dev_reload()                     // /__quarlus_dev/* endpoints
    .with_openapi(openapi_config)          // /openapi.json (+ /docs if docs_ui enabled)
    .with_scheduler(|s| {
        s.register::<ScheduledJobs>();
    })
    .on_start(|state| async move { Ok(()) })
    .on_stop(|| async { })
    .register_controller::<UserController>()
    .register_controller::<AccountController>()
    .build()                               // → axum::Router
    // or .serve("0.0.0.0:3000").await     // build + listen + graceful shutdown
```

`build()` returns an `axum::Router`. `serve(addr)` builds, runs startup hooks, registers event consumers, starts listening, waits for shutdown signal (Ctrl-C / SIGTERM), then runs shutdown hooks.

### Testing (quarlus-test)

- `TestApp` — wraps an `axum::Router` with an HTTP client for integration testing. Methods: `get`, `post`, `put`, `delete`, `patch` with builder pattern for headers/body.
- `TestResponse` — response wrapper with status, headers, and body helpers.
- `TestJwt` — generates valid JWT tokens for test scenarios with configurable sub/email/roles.

### Configuration (quarlus-core)

`QuarlusConfig` — key-value configuration store loaded from YAML files + environment variable overlay.
- `QuarlusConfig::load("dev")` — load `application.yaml`, then `application-dev.yaml`, then overlay env vars. Profile overridable via `QUARLUS_PROFILE` env var.
- `QuarlusConfig::empty()` — empty config for testing.
- `config.set("key", ConfigValue::String("value".into()))` — manual key-value setup.
- `config.get::<T>("key")` — retrieve a typed value (`T: FromConfigValue`).
- `config.get_or("key", default)` — retrieve with fallback.
- `#[config("app.key")]` field attribute on controllers — injected at request time from the config stored in state.

### Feature Flags

- `quarlus-core` has an optional `sqlx` feature that enables `sqlx::Error` → `AppError` conversion.
- `quarlus-core` has an optional `validation` feature that enables the `Validated<T>` extractor.
- `#[transactional]` attribute (in macros) wraps a method body in `self.pool.begin()`/`commit()` — requires the controller to have an injected `pool` field.

## Language & Documentation

The project's plan (`plan.md`) and step-by-step docs (`docs/steps/`) are written in French. Code, comments, and API surfaces are in English.
