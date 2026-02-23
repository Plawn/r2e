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
cargo build -p r2e-core
cargo build -p r2e-macros
cargo build -p r2e-security
cargo build -p r2e-events
cargo build -p r2e-scheduler
cargo build -p r2e-data
cargo build -p r2e-cache
cargo build -p r2e-rate-limit
cargo build -p r2e-openapi
cargo build -p r2e-utils
cargo build -p r2e-test
cargo build -p r2e-cli

# Expand macros for debugging (requires cargo-expand)
cargo expand -p example-app
```

## Testing Conventions

**Tests live in `<crate>/tests/` directories, not inline.** Do NOT use `#[cfg(test)] mod tests { ... }` blocks inside source files. Instead, create separate test files under the crate's `tests/` directory.

### Rules

- One test file per source module: `src/foo.rs` → `tests/foo.rs`
- Use external imports (`use <crate_name>::...`) instead of `use super::*`
- Keep test-only helpers (structs, functions) in the test file, not in the source
- If a test needs access to an internal item, add a `pub` accessor or make the item `pub` + `#[doc(hidden)]` — do NOT use `#[cfg(test)] pub(crate)` visibility hacks
- Feature-gated modules need `#![cfg(feature = "...")]` at the top of the test file (e.g., `ws.rs` tests use `#![cfg(feature = "ws")]`)

### Example

```rust
// tests/config.rs
use r2e_core::config::{R2eConfig, ConfigValue};

#[test]
fn test_get_or_returns_default() {
    let config = R2eConfig::empty();
    assert_eq!(config.get_or("missing", "fallback".to_string()), "fallback");
}
```

### Running tests

```bash
cargo test --workspace           # all tests
cargo test -p r2e-core           # single crate
cargo test -p r2e-core --test config  # single test file
```

## Architecture

R2E is a **Quarkus-like ergonomic layer over Axum** for Rust. It provides declarative controllers with compile-time dependency injection, JWT/OIDC security, and zero runtime reflection.

### Workspace Crates

```
r2e-macros      → Proc-macro crate (no runtime deps). #[derive(Controller)] + #[routes] generate Axum handlers.
r2e-core        → Runtime foundation. AppBuilder, Controller trait, StatefulConstruct trait, HttpError, Guard trait,
                      Interceptor trait, R2eConfig, lifecycle hooks, Tower layers, dev-mode endpoints.
r2e-security    → JWT validation, JWKS cache, AuthenticatedUser extractor, RoleExtractor trait.
r2e-events      → In-process EventBus with typed pub/sub (emit, emit_and_wait, subscribe).
r2e-scheduler   → Background task scheduling (interval, cron, initial delay). CancellationToken-based shutdown.
r2e-data        → Data access abstractions: Entity, Repository, Page, Pageable, DataError (no driver deps).
r2e-data-sqlx   → SQLx backend: SqlxRepository, Tx, HasPool, ManagedResource impl, error bridge, migrations.
r2e-data-diesel → Diesel backend (skeleton): DieselRepository, error bridge.
r2e-cache       → TtlCache, pluggable CacheStore trait (default InMemoryStore), global cache backend singleton.
r2e-rate-limit  → Token-bucket RateLimiter, pluggable RateLimitBackend trait, RateLimitRegistry, RateLimitGuard.
r2e-openapi     → OpenAPI 3.0.3 spec generation from route metadata, Swagger UI at /docs.
r2e-utils       → Built-in interceptors: Logged, Timed, Cache, CacheInvalidate.
r2e-test        → Test helpers: TestApp (HTTP client wrapper), TestJwt (JWT generation for tests).
r2e-cli         → CLI tool: r2e new, r2e add, r2e dev, r2e generate, r2e doctor, r2e routes.
example-app         → Demo binary exercising all features.
```

Dependency flow: `r2e-macros` ← `r2e-core` ← `r2e-security` / `r2e-events` / `r2e-scheduler` / `r2e-data` ← `r2e-data-sqlx` / `r2e-data-diesel` / `r2e-cache` / `r2e-rate-limit` / `r2e-openapi` / `r2e-utils` / `r2e-test` ← `example-app`

### Vendored Dependencies

`vendor/openfga-rs/` — patched copy of the upstream git version of `openfga-rs`. The crates.io release (0.1.0) uses tonic ~0.11 which cannot separate the gRPC client (`channel`) from `server`/`router`/axum, causing a dual axum-core conflict with r2e's axum 0.8. The vendored copy uses tonic ~0.12 with `default-features = false, features = ["tls", "channel", "codegen", "prost"]` to get the client without pulling in axum. See `vendor/README.md` for full details. The workspace `[patch.crates-io]` section points to this directory.

### Core Concepts

**Three injection scopes, all resolved at compile time:**
- `#[inject]` — App-scoped. Field is cloned from the Axum state (services, repos, pools). Type must be `Clone + Send + Sync`.
- `#[inject(identity)]` — Request-scoped. Field is extracted via Axum's `FromRequestParts` (e.g., `AuthenticatedUser` from JWT). Type must implement `Identity`. Legacy `#[identity]` syntax is still supported.
- `#[config("key")]` — App-scoped. Field is resolved from `R2eConfig` at request time. Field type must implement `FromConfigValue` (`String`, `i64`, `f64`, `bool`, `Option<T>`).

**Handler parameter-level identity injection:**
- `#[inject(identity)]` can also be placed on handler parameters (not just struct fields). This enables mixed controllers where some endpoints are public and others require authentication, while still allowing `StatefulConstruct` generation for consumers and scheduled tasks.
- **Optional identity:** Use `#[inject(identity)] user: Option<AuthenticatedUser>` for endpoints that work both with and without authentication. The guard context will receive `None` when no JWT is present.

**Controller declaration uses two macros working together:**

1. `#[derive(Controller)]` on the struct — generates metadata module, Axum extractor, and `StatefulConstruct` impl (when no identity fields).
2. `#[routes]` on the impl block — generates Axum handler functions and `Controller<T>` trait impl (including scheduled task definitions).

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
- `mod __r2e_meta_<Name>` — contains `type State`, `type IdentityType`, `const PATH_PREFIX`, `fn guard_identity()`
- `struct __R2eExtract_<Name>` — `FromRequestParts` extractor that constructs the controller from state + request parts
- `impl StatefulConstruct<State> for Name` — only when no `#[inject(identity)]` struct fields; used by consumers and scheduled tasks
- Free-standing Axum handler functions (named `__r2e_<Name>_<method>`)
- `impl Controller<State> for Name` — wires routes into `axum::Router<State>`

### Macro Crate Internals (r2e-macros)

The proc-macro pipeline has two entry points:

**Derive path:** `lib.rs` → `derive_controller.rs` → `derive_parsing.rs` (DeriveInput → `ControllerStructDef`) → `derive_codegen.rs` (generate meta module, extractor, StatefulConstruct)

**Routes path:** `lib.rs` → `routes_attr.rs` → `routes_parsing.rs` (ItemImpl → `RoutesImplDef`) → `routes_codegen.rs` (generate impl block, handlers, Controller trait impl with scheduled_tasks)

**Shared modules:**
- `types.rs` — shared types (`InjectedField`, `IdentityField`, `ConfigField`, `RouteMethod`, `ConsumerMethod`, `ScheduledMethod`, etc.)
- `attr_extract.rs` — attribute extraction functions (`extract_route_attr`, `extract_roles`, `extract_transactional`, `extract_intercept_fns`, etc.)
- `route.rs` — `HttpMethod` enum and `RoutePath` parser

**Inter-macro liaison:** The derive generates a hidden module `__r2e_meta_<Name>` and an extractor struct `__R2eExtract_<Name>`. The `#[routes]` macro references these by naming convention.

Handler generation pattern: each `#[get("/path")]` method becomes a standalone async function that takes `__R2eExtract_<Name>` (which implements `FromRequestParts`) and method parameters. The extractor constructs the controller from state + request parts. For guarded handlers, `State(state)` and `HeaderMap` are also extracted.

**No-op attribute macros:** `lib.rs` declares attributes like `#[get]`, `#[roles]`, `#[intercept]`, `#[guard]`, `#[consumer]`, `#[scheduled]`, `#[middleware]`, etc. as no-op `#[proc_macro_attribute]` that return their input unchanged. These are parsed from the token stream by `#[routes]`. The no-op declarations exist for: (1) preventing "cannot find attribute" errors outside `#[routes]`, (2) `cargo doc` visibility, (3) IDE autocomplete support. The `#[inject]`, `#[identity]`, and `#[config]` attributes are derive helper attributes (consumed by `#[derive(Controller)]`). Note: `#[inject(identity)]` on handler parameters is parsed and stripped by `#[routes]` macro processing.

### Guards

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

**Built-in guards:**
- `RolesGuard` — checks required roles, returns 403 if missing. Applied via `#[roles("admin")]`. Implements `Guard<S, I>` for any `I: Identity`.
- `RateLimitGuard` / `PreAuthRateLimitGuard` — token-bucket rate limiting, returns 429. Use the `RateLimit` builder with `#[guard(...)]` or `#[pre_guard(...)]`:
  ```rust
  use r2e::r2e_rate_limit::RateLimit;

  #[pre_guard(RateLimit::global(5, 60))]    // 5 req / 60 sec, shared bucket (pre-auth)
  #[pre_guard(RateLimit::per_ip(5, 60))]    // 5 req / 60 sec, per IP (pre-auth)
  #[guard(RateLimit::per_user(5, 60))]      // 5 req / 60 sec, per user (post-auth)
  ```

**Pre-authentication guards:**

For authorization checks that don't require identity (e.g., IP-based rate limiting, allowlisting), use the `PreAuthGuard<S>` trait. Pre-auth guards run as middleware **before** JWT extraction, avoiding wasted token validation when requests will be rejected.

- `PreAuthGuardContext` — provides `method_name`, `controller_name`, `headers`, `uri` (no identity)
- `PreAuthRateLimitGuard` — pre-auth rate limiter for global/IP keys
- Apply custom pre-auth guards via `#[pre_guard(MyPreAuthGuard)]`

**Rate-limiting key classification:**
- `RateLimit::global()` / `RateLimit::per_ip()` → use with `#[pre_guard(...)]` (runs before JWT validation)
- `RateLimit::per_user()` → use with `#[guard(...)]` (runs after JWT validation, needs identity)

**Custom guards:**
- Post-auth: implement `Guard<S, I: Identity>` (async via RPITIT) and apply via `#[guard(MyGuard)]`
- Pre-auth: implement `PreAuthGuard<S>` and apply via `#[pre_guard(MyPreAuthGuard)]`

**Async guard example:**
```rust
struct DatabaseGuard;

impl<S: Send + Sync, I: Identity> Guard<S, I> for DatabaseGuard
where
    sqlx::SqlitePool: FromRef<S>,
{
    fn check(
        &self,
        state: &S,
        ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async move {
            let pool = sqlx::SqlitePool::from_ref(state);
            // Async database check...
            sqlx::query("SELECT 1").fetch_one(&pool).await
                .map_err(|_| HttpError::Internal("DB unavailable".into()).into_response())?;
            Ok(())
        }
    }
}
```

### Interceptors

Cross-cutting concerns (logging, timing, caching) are implemented via a generic `Interceptor<R>` trait with an `around` pattern (`r2e-core/src/interceptors.rs`). All calls are monomorphized (no `dyn`) for zero overhead.

**Built-in interceptors** (in `r2e-utils`):
- `Logged` — logs entry/exit at a configurable `LogLevel`.
- `Timed` — measures execution time, with an optional threshold (only logs if exceeded).
- `Cache` — caches `Json<T>` responses via the global `CacheStore`. Supports TTL and named groups.
- `CacheInvalidate` — clears a named cache group after method execution.

**Interceptor wrapping order** (outermost → innermost):

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

**Configurable syntax:**
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

### Cache (r2e-cache)

`TtlCache<K, V>` — thread-safe TTL cache backed by `DashMap`. Supports get, insert, remove, clear, evict_expired.

`CacheStore` trait — pluggable async cache backend. Default: `InMemoryStore` (DashMap-backed). Supports get, set, remove, clear, remove_by_prefix. Global singleton via `set_cache_backend()` / `cache_backend()`.

The `Cache` interceptor (in `r2e-utils`) uses the global `CacheStore` backend. `#[intercept(Cache::ttl(30).group("users"))]` stores in a named group; `#[intercept(CacheInvalidate::group("users"))]` clears by prefix.

### Rate Limiting (r2e-rate-limit)

`RateLimiter<K>` — generic token-bucket rate limiter keyed by arbitrary type. `RateLimitBackend` trait for pluggable backends (default: `InMemoryRateLimiter`). `RateLimitRegistry` — clonable handle stored in app state, used by the generated `RateLimitGuard`.

Key kinds: `"global"` (shared bucket), `"user"` (per authenticated user sub), `"ip"` (per X-Forwarded-For).

### Security (r2e-security)

- `AuthenticatedUser` implements `FromRequestParts` and `Identity` — extracts Bearer token, validates via `JwtValidator`, returns user with sub/email/roles/claims.
- `JwtValidator` supports both static keys (testing) and JWKS endpoint (production) via `JwksCache`.
- `SecurityConfig` — configuration for JWT validation (issuer, audience, JWKS URL, static keys).
- `#[roles("admin")]` attribute generates a guard that checks identity roles via the `Identity` trait and returns 403 if missing.
- Role extraction is trait-based (`RoleExtractor`) to support multiple OIDC providers; default (`DefaultRoleExtractor`) checks top-level `roles` and Keycloak's `realm_access.roles`.

### Events (r2e-events)

`EventBus` — in-process typed pub/sub. Events are dispatched by `TypeId`. Subscribers receive `Arc<E>`.

- `bus.subscribe(|event: Arc<MyEvent>| async { ... })` — register a handler.
- `bus.emit(event)` — fire-and-forget (spawns handlers as concurrent tasks).
- `bus.emit_and_wait(event)` — waits for all handlers to complete.

**Declarative consumers** via `#[consumer(bus = "field_name")]` in a `#[routes]` impl block. The controller must not have `#[inject(identity)]` struct fields (requires `StatefulConstruct`). Consumers are registered automatically by `AppBuilder::register_controller`.

### Scheduling (r2e-scheduler)

Scheduled tasks are auto-discovered via `register_controller()`, following the same pattern as event consumers. The scheduler runtime (`r2e-scheduler`) provides the `Scheduler` plugin (unit struct) that installs `CancellationToken`-based lifecycle management.

**Schedule data types** (in `r2e-core::scheduling`, zero new deps):
- `ScheduleConfig::Interval(duration)` — fixed interval.
- `ScheduleConfig::IntervalWithDelay { interval, initial_delay }` — with initial delay.
- `ScheduleConfig::Cron(expr)` — cron expression (via `cron` crate in the runtime).
- `ScheduledTaskDef<T>` — a named task definition with schedule and closure.
- `ScheduledResult` — trait for handling `()` or `Result<(), E>` return values.

**Declarative scheduling** via `#[scheduled]` attribute:
```rust
#[scheduled(every = 30)]                    // every 30 seconds
#[scheduled(every = 60, delay = 10)]        // first run after 10s
#[scheduled(cron = "0 */5 * * * *")]        // cron expression
```

**Registration:** install the `Scheduler` plugin before `build_state()`, then register controllers:
```rust
AppBuilder::new()
    .plugin(Scheduler)                        // install scheduler runtime (provides CancellationToken)
    .build_state::<Services, _, _>()
    .await
    .register_controller::<ScheduledJobs>()   // auto-discovers #[scheduled] methods
    .serve("0.0.0.0:3000")
```

The `Controller` trait's `scheduled_tasks()` method (auto-generated by `#[routes]`) returns `Vec<ScheduledTaskDef<T>>`. `register_controller()` collects these. `serve()` passes them to the scheduler backend, which spawns Tokio tasks. On shutdown, the `CancellationToken` is cancelled.

Controllers with `#[inject(identity)]` struct fields cannot be used for scheduling (no `StatefulConstruct` impl). Controllers using param-level `#[inject(identity)]` only retain `StatefulConstruct` and can be used for scheduling.

### Data (r2e-data)

- `Entity` trait — maps a Rust struct to a SQL table (table name, column list).
- `QueryBuilder` — fluent SQL query builder (`where_eq`, `where_like`, `order_by`, `limit`, `offset`).
- `Repository` trait — async CRUD interface (`find_by_id`, `find_all`, `create`, `update`, `delete`).
- `SqlxRepository` — SQLx-backed implementation of `Repository`.
- `Pageable` — pagination parameters extracted from query string (page, size, sort).
- `Page<T>` — paginated response wrapper (content, total_elements, total_pages, page, size).
- `DataError` — data-layer error type.

### OpenAPI (r2e-openapi)

- `OpenApiConfig` — configuration for the generated spec (title, version, description). `with_docs_ui(true)` enables the interactive documentation page.
- `AppBuilderOpenApiExt::with_openapi(config)` — registers OpenAPI routes.
- `SchemaRegistry` / `SchemaProvider` — JSON Schema collection for request/response types.
- Route metadata is collected from `Controller::route_metadata()` during `register_controller`.
- Always serves the spec at `/openapi.json`. When `docs_ui` is enabled, also serves an interactive API documentation UI at `/docs`.

### StatefulConstruct (r2e-core)

`StatefulConstruct<S>` trait allows constructing a controller from state alone (no HTTP context). Auto-generated by `#[derive(Controller)]` when the struct has no `#[inject(identity)]` fields. Used by:
- Consumer methods (`#[consumer]`) — event handlers that run outside HTTP requests
- Scheduled methods (`#[scheduled]`) — background tasks

Controllers with `#[inject(identity)]` struct fields do NOT get this impl. Attempting to use them in consumer/scheduled context produces a compile error with a diagnostic message via `#[diagnostic::on_unimplemented]`. Controllers using param-level `#[inject(identity)]` only retain `StatefulConstruct` — this is the key advantage of the mixed controller pattern.

### AppBuilder (r2e-core)

Fluent API for assembling a R2E application:

```rust
AppBuilder::new()
    .plugin(Scheduler)                     // scheduler runtime - MUST be before build_state()
    .provide(services.pool.clone())        // provide beans
    .with_producer::<CreatePool>()         // async producer (registers SqlitePool)
    .with_async_bean::<MyAsyncService>()   // async bean constructor
    .with_bean::<UserService>()            // sync bean (unchanged)
    .build_state::<Services, _, _>()       // resolve bean graph (async — .await required)
    .await
    .with_config(config)
    .with(Health)                          // /health → 200 "OK"
    .with(Cors::permissive())              // or Cors::new(custom_layer)
    .with(Tracing)
    .with(ErrorHandling)                   // catch panics → JSON 500
    .with(DevReload)                       // /__r2e_dev/* endpoints
    .with(OpenApiPlugin::new(config))      // /openapi.json (+ /docs if docs_ui enabled)
    .on_start(|state| async move { Ok(()) })
    .on_stop(|| async { })
    .register_controller::<UserController>()
    .register_controller::<AccountController>()
    .register_controller::<ScheduledJobs>() // auto-discovers #[scheduled] methods
    .build()                               // → axum::Router
    // or .serve("0.0.0.0:3000").await     // build + listen + graceful shutdown
```

`build()` returns an `axum::Router`. `serve(addr)` builds, runs startup hooks, registers event consumers, starts scheduled tasks, starts listening, waits for shutdown signal (Ctrl-C / SIGTERM), stops the scheduler, then runs shutdown hooks.

### Testing (r2e-test)

- `TestApp` — wraps an `axum::Router` with an HTTP client for integration testing. Methods: `get`, `post`, `put`, `delete`, `patch` with builder pattern for headers/body.
- `TestResponse` — response wrapper with status, headers, and body helpers.
- `TestJwt` — generates valid JWT tokens for test scenarios with configurable sub/email/roles.

### Configuration (r2e-core)

`R2eConfig` — key-value configuration store loaded from YAML files + environment variable overlay.
- `R2eConfig::load("dev")` — load `application.yaml`, then `application-dev.yaml`, then overlay env vars. Profile overridable via `R2E_PROFILE` env var.
- `R2eConfig::empty()` — empty config for testing.
- `config.set("key", ConfigValue::String("value".into()))` — manual key-value setup.
- `config.get::<T>("key")` — retrieve a typed value (`T: FromConfigValue`).
- `config.get_or("key", default)` — retrieve with fallback.
- `#[config("app.key")]` field attribute on controllers — injected at request time from the config stored in state.

### Managed Resources (r2e-core)

The `#[managed]` attribute enables automatic lifecycle management for resources like database transactions, connections, scoped caches, or audit contexts. Resources are acquired before handler execution and released after, with success/failure status.

**Core trait:**
```rust
pub trait ManagedResource<S>: Sized {
    type Error: Into<Response>;

    async fn acquire(state: &S) -> Result<Self, Self::Error>;
    async fn release(self, success: bool) -> Result<(), Self::Error>;
}
```

**Usage with `#[managed]`:**
```rust
#[routes]
impl UserController {
    #[post("/")]
    async fn create(
        &self,
        body: Json<User>,
        #[managed] tx: &mut Tx<'_, Sqlite>,  // Acquired before, released after
    ) -> Result<Json<User>, MyHttpError> {
        sqlx::query("INSERT INTO users ...").execute(tx.as_mut()).await?;
        Ok(Json(user))
    }
}
```

**Lifecycle:**
1. `acquire(&state)` — called before handler, resource obtained from app state
2. Handler receives `&mut Resource`
3. `release(self, success)` — called after handler
   - `success = true` if handler returned `Ok` or non-Result type
   - `success = false` if handler returned `Err`

**Example: Transaction wrapper (user-defined):**
```rust
// Define the wrapper type
pub struct Tx<'a, DB: Database>(pub Transaction<'a, DB>);

// Define how to get a pool from state
pub trait HasPool<DB: Database> {
    fn pool(&self) -> &Pool<DB>;
}

// Implement ManagedResource
impl<S, DB> ManagedResource<S> for Tx<'static, DB>
where
    DB: Database,
    S: HasPool<DB> + Send + Sync,
{
    type Error = ManagedErr<MyHttpError>;

    async fn acquire(state: &S) -> Result<Self, Self::Error> {
        let tx = state.pool().begin().await
            .map_err(|e| MyHttpError::Database(e.to_string()))?;
        Ok(Tx(tx))
    }

    async fn release(self, success: bool) -> Result<(), Self::Error> {
        if success {
            self.0.commit().await
                .map_err(|e| MyHttpError::Database(e.to_string()))?;
        }
        // On failure: transaction dropped → automatic rollback
        Ok(())
    }
}
```

**Note:** `#[managed]` and `#[transactional]` are mutually exclusive. Prefer `#[managed]` for new code as it's more flexible and explicit.

### Error Handling (r2e-core)

R2E provides `HttpError` as a default error type, `#[derive(ApiError)]` for custom error types, and automatic validation error handling.

**`HttpError` variants:**

| Variant | Status | Body |
|---------|--------|------|
| `NotFound(String)` | 404 | `{"error": "..."}` |
| `Unauthorized(String)` | 401 | `{"error": "..."}` |
| `Forbidden(String)` | 403 | `{"error": "..."}` |
| `BadRequest(String)` | 400 | `{"error": "..."}` |
| `Internal(String)` | 500 | `{"error": "..."}` |
| `Validation(ValidationErrorResponse)` | 400 | `{"error": "Validation failed", "details": [...]}` |
| `Custom { status, body }` | any | custom JSON body |

**Using the built-in `HttpError`:**
```rust
use r2e_core::HttpError;

#[get("/{id}")]
async fn get(&self, Path(id): Path<i64>) -> Result<Json<User>, HttpError> {
    let user = self.service.find(id).await
        .ok_or_else(|| HttpError::NotFound("User not found".into()))?;
    Ok(Json(user))
}
```

**`map_error!` macro** — bulk `From<E> for HttpError` generation:
```rust
r2e_core::map_error! {
    sqlx::Error => Internal,
    serde_json::Error => BadRequest,
}
```

**`#[derive(ApiError)]` — recommended for custom error types:**

Generates `Display`, `IntoResponse`, and `std::error::Error` impls automatically. Available in the prelude.

```rust
#[derive(Debug, ApiError)]
pub enum MyError {
    #[error(status = NOT_FOUND, message = "User not found: {0}")]
    NotFound(String),

    #[error(status = INTERNAL_SERVER_ERROR)]
    Io(#[from] std::io::Error),

    #[error(status = BAD_REQUEST)]
    Validation(String),

    #[error(status = CONFLICT)]
    AlreadyExists,

    #[error(status = 429, message = "Too many requests")]
    RateLimited,

    #[error(status = BAD_REQUEST, message = "Field {field} is invalid: {reason}")]
    InvalidField { field: String, reason: String },

    #[error(transparent)]
    Http(#[from] HttpError),
}
```

Attribute syntax on variants:
- `#[error(status = NAME, message = "...")]` — explicit status + message with `{0}`/`{field}` interpolation
- `#[error(status = NAME)]` — status only; message inferred (String field value, `#[from]` source `.to_string()`, or humanized variant name for units)
- `#[error(status = 429)]` — numeric status code
- `#[error(transparent)]` — delegates Display + IntoResponse to the inner type
- `#[from]` on a field — generates `From<T>` impl and `Error::source()` returns that field

**Key files:** `r2e-core/src/error.rs` (HttpError, `error_response()`, `map_error!`), `r2e-macros/src/api_error_derive.rs` (derive implementation), `r2e-core/tests/api_error.rs` (comprehensive tests)

**Validation errors (`HttpError::Validation`):**

Produced automatically by the `garde` integration. When `Json<T>` is extracted and `T: garde::Validate`, validation runs before the handler body. On failure, a 400 response is returned:
```json
{"error": "Validation failed", "details": [{"field": "email", "message": "not a valid email", "code": "validation"}]}
```
The underlying types: `ValidationErrorResponse { errors: Vec<FieldError> }` and `FieldError { field, message, code }` (in `r2e-core::validation`). The validation uses an autoref specialization trick (`__AutoValidator` / `__DoValidate` / `__SkipValidate`) so types without `Validate` have zero overhead.

**Manual custom error types (without derive):**

You can also implement `IntoResponse` manually:
```rust
#[derive(Debug)]
pub enum MyHttpError {
    NotFound(String),
    Database(String),
}

impl IntoResponse for MyHttpError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            MyHttpError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            MyHttpError::Database(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        let body = serde_json::json!({ "error": message });
        (status, Json(body)).into_response()
    }
}
```

**Error wrappers for `ManagedResource`:**

The `ManagedResource` trait requires `Error: Into<Response>`. Due to Rust's orphan rules, you can't implement `Into<Response>` directly for your error type. R2E provides two wrappers:

- `ManagedError` — wraps the built-in `HttpError`
- `ManagedErr<E>` — generic wrapper for any error type implementing `IntoResponse`

```rust
use r2e_core::{ManagedResource, ManagedErr};

impl<S: HasPool + Send + Sync> ManagedResource<S> for Tx<'static, Sqlite> {
    type Error = ManagedErr<MyHttpError>;  // Use your custom error

    async fn acquire(state: &S) -> Result<Self, Self::Error> {
        let tx = state.pool().begin().await
            .map_err(|e| ManagedErr(MyHttpError::Database(e.to_string())))?;
        Ok(Tx(tx))
    }
    // ...
}
```

**Why `ManagedErr<E>` is needed:**

Rust's orphan rules prevent implementing foreign traits (`Into`) for foreign types (`Response`). `ManagedErr<E>` is a local newtype that bridges the gap:

```
MyHttpError (your type)     →  ManagedErr<MyHttpError> (r2e type)  →  Response (axum type)
         impl IntoResponse              impl Into<Response>
```

### Prelude & HTTP Re-exports (r2e-core)

**`use r2e::prelude::*`** provides everything a developer needs — no direct `axum` imports should be necessary. The prelude includes:

- **Macros:** `Controller`, `routes`, `get`/`post`/`put`/`delete`/`patch`, `guard`, `intercept`, `roles`, `managed`, `transactional`, `consumer`, `scheduled`, `bean`, `producer`, `Bean`, `BeanState`, `Params`, `ConfigProperties`, `Cacheable`, `ApiError`, `FromMultipart` (multipart feature)
- **Core types:** `AppBuilder`, `HttpError`, `R2eConfig`, `ConfigValue`, `Plugin`, `Interceptor`, `ManagedResource`, `ManagedErr`, `Guard`, `GuardContext`, `Identity`, `PreAuthGuard`, `StatefulConstruct`
- **Plugins:** `Cors`, `Tracing`, `Health`, `ErrorHandling`, `DevReload`, `NormalizePath`, `SecureHeaders`, `RequestIdPlugin`
- **HTTP core:** `Json`, `Router`, `StatusCode`, `HeaderMap`, `Uri`, `Extension`, `Body`, `Bytes`
- **Extractors:** `Path`, `Query`, `Form`, `State`, `Request`, `FromRef`, `FromRequest`, `FromRequestParts`, `ConnectInfo`, `DefaultBodyLimit`, `MatchedPath`, `OriginalUri`
- **Headers:** `HeaderName`, `HeaderValue`, `Method`, plus constants: `ACCEPT`, `AUTHORIZATION`, `CACHE_CONTROL`, `CONTENT_LENGTH`, `CONTENT_TYPE`, `COOKIE`, `HOST_HEADER`, `LOCATION`, `ORIGIN`, `REFERER`, `SET_COOKIE`, `USER_AGENT`
- **Response:** `IntoResponse`, `Response`, `Redirect`, `Html`, `Sse`, `SseEvent`, `SseKeepAlive`, `SseBroadcaster`
- **Middleware:** `from_fn`, `Next`
- **Type aliases:** `ApiResult`, `JsonResult`, `StatusResult`
- **Multipart** (feature `multipart`): `Multipart`, `TypedMultipart`, `UploadedFile`, `FromMultipart`
- **WebSocket** (feature `ws`): `WebSocket`, `WebSocketUpgrade`, `Message`, `CloseFrame`, `WsStream`, `WsHandler`, `WsBroadcaster`, `WsRooms`

Additional types are available via `r2e::http::*` submodules for advanced use (e.g., `r2e::http::routing::{get, post, ...}`, `r2e::http::body::Body`).

### Feature Flags

- Validation uses `garde` crate and is always available (no feature flag). Types deriving `garde::Validate` are automatically validated when extracted via `Json<T>`.
- `#[derive(Params)]` aggregates path, query, and header params into a single DTO (BeanParam-like). Also generates `ParamsMetadata` for automatic OpenAPI parameter documentation.
- `#[transactional]` attribute (in macros) wraps a method body in `self.pool.begin()`/`commit()` — requires the controller to have an injected `pool` field. Consider using `#[managed]` instead for more flexibility.

### Beans & Dependency Injection (r2e-core, r2e-macros)

**Three bean traits** for the dependency graph:

| Trait | Constructor | Registration | Use case |
|-------|-----------|-------------|----------|
| `Bean` | `fn build(ctx) -> Self` (sync) | `.with_bean::<T>()` | Simple services |
| `AsyncBean` | `async fn build(ctx) -> Self` | `.with_async_bean::<T>()` | Services needing async init |
| `Producer` | `async fn produce(ctx) -> Output` | `.with_producer::<P>()` | Types you don't own (pools, clients) |

All three traits have an associated `type Deps` that declares their dependencies as a type-level list (e.g., `type Deps = TCons<EventBus, TNil>`). This is generated automatically by the `#[bean]`, `#[derive(Bean)]`, and `#[producer]` macros. For manual impls without dependencies, use `type Deps = TNil;`.

**`build_state()` is async** — it must be `.await`ed because the bean graph may contain async beans or producers. It takes 3 generic args: `build_state::<S, _, _>()` (state type, provisions, requirements).

**`#[bean]` attribute macro** — auto-detects sync vs async constructors:

```rust
// Sync → generates `impl Bean`
#[bean]
impl UserService {
    fn new(event_bus: EventBus) -> Self { Self { event_bus } }
}

// Async → generates `impl AsyncBean`
#[bean]
impl MyAsyncService {
    async fn new(pool: SqlitePool) -> Self { /* ... */ Self { pool } }
}
```

**`#[producer]` attribute macro** — for free functions producing types you don't own:

```rust
#[producer]
async fn create_pool(#[config("app.db.url")] url: String) -> SqlitePool {
    SqlitePool::connect(&url).await.unwrap()
}
// Generates: struct CreatePool; impl Producer for CreatePool { type Output = SqlitePool; ... }
```

**`#[config("key")]` in beans** — resolve values from `R2eConfig` instead of the bean graph:

```rust
// In #[bean] constructor params:
#[bean]
impl NotificationService {
    fn new(bus: EventBus, #[config("notification.capacity")] capacity: i64) -> Self { ... }
}

// In #[derive(Bean)] fields:
#[derive(Clone, Bean)]
struct MyService {
    #[inject] event_bus: EventBus,
    #[config("app.name")] name: String,
}
```

When `#[config]` is used, `R2eConfig` is automatically added to the dependency list. Missing config keys panic with a message including the env var equivalent (e.g., `APP_DB_URL`).

**Key files:**
- `r2e-core/src/beans.rs` — `Bean`, `AsyncBean`, `Producer`, `BeanContext`, `BeanRegistry`
- `r2e-core/src/builder.rs` — `with_bean()`, `with_async_bean()`, `with_producer()`, async `build_state()`
- `r2e-macros/src/bean_attr.rs` — `#[bean]` (sync + async detection, `#[config]` param support)
- `r2e-macros/src/bean_derive.rs` — `#[derive(Bean)]` (`#[inject]` + `#[config]` field support)
- `r2e-macros/src/producer_attr.rs` — `#[producer]` macro

### CLI (r2e-cli)

The `r2e` binary provides project scaffolding, code generation, diagnostics, and development tooling.

**Key files:**
- `r2e-cli/src/main.rs` — CLI entry point (clap `Commands` + `GenerateKind` enums)
- `r2e-cli/src/commands/` — one module per command
- `r2e-cli/src/commands/templates/` — code generation templates (project, middleware)

#### `r2e new <name>` — Project scaffolding

Creates a new R2E project with optional feature selection.

**Flags:**
- `--db <sqlite|postgres|mysql>` — include database support (adds sqlx dep, pool in state, migrations/ dir)
- `--auth` — include JWT/OIDC security (adds `r2e-security`, `JwtClaimsValidator` in state)
- `--openapi` — include OpenAPI documentation (adds `OpenApiPlugin` to builder)
- `--metrics` — reserved for Prometheus metrics (not yet wired)
- `--full` — enable all features (SQLite + auth + openapi + scheduler + events)
- `--no-interactive` — skip interactive prompts, use flags/defaults only

**Interactive mode:** When no flags are provided, uses `dialoguer` to prompt for database and feature selection.

**Generated project uses the `r2e` facade crate** (not `r2e-core` + `r2e-macros` separately). Templates are in `commands/templates/project.rs`.

**Types:**
- `ProjectOptions` — aggregates all feature selections
- `DbKind` — `Sqlite | Postgres | Mysql`
- `CliNewOpts` — raw CLI flag values before resolution

#### `r2e generate` — Code generation

Subcommands:

- **`controller <Name>`** — generates `src/controllers/<snake_name>.rs` with a skeleton controller, updates `mod.rs`
- **`service <Name>`** — generates `src/<snake_name>.rs` with a skeleton service struct
- **`crud <Name> --fields "name:Type ..."`** — generates a complete CRUD set:
  - `src/models/<snake>.rs` — entity struct + `Create`/`Update` request types
  - `src/services/<snake>_service.rs` — service with list/get/create/update/delete methods
  - `src/controllers/<snake>_controller.rs` — REST controller with GET/POST/PUT/DELETE endpoints
  - `migrations/<timestamp>_create_<plural>.sql` — SQL migration (if `migrations/` dir exists)
  - `tests/<snake>_test.rs` — integration test skeleton
  - Updates `mod.rs` in each directory
- **`middleware <Name>`** — generates `src/middleware/<snake_name>.rs` with an `Interceptor<R>` impl skeleton, updates `mod.rs`

**Field parsing:** fields are `"name:Type"` pairs (e.g. `"title:String published:bool"`). `Field` struct has `name`, `rust_type`, `is_optional`. SQL type mapping: `String` → `TEXT`, `i64` → `INTEGER`, `f64` → `REAL`, `bool` → `BOOLEAN`.

#### `r2e doctor` — Project health diagnostics

Runs 8 checks against the current working directory and reports issues:

| Check | Level | What it verifies |
|-------|-------|------------------|
| Cargo.toml exists | Error | Current dir is a Rust project |
| R2E dependency | Error | `r2e` appears in Cargo.toml dependencies |
| Configuration file | Warning | `application.yaml` exists |
| Controllers directory | Warning | `src/controllers/` exists, counts `.rs` files |
| Rust toolchain | Error | `rustc --version` succeeds |
| cargo-watch | Warning | `cargo watch --version` succeeds (needed for `r2e dev`) |
| Migrations directory | Warning | If data feature is used, `migrations/` dir exists |
| Application entrypoint | Warning | `src/main.rs` contains a `.serve()` call |

Each check returns `Ok`, `Warning`, or `Error` with a colored indicator (`✓` / `!` / `x`).

#### `r2e routes` — Route listing

Static source parsing of `src/controllers/*.rs` (no compilation required). For each controller file:
1. Extracts `#[controller(path = "...")]` base path
2. Finds `#[get("/...")]`, `#[post]`, `#[put]`, `#[delete]`, `#[patch]` attributes
3. Resolves the handler name from the next `fn` declaration
4. Captures `#[roles("...")]` if present

Output is a colored table sorted by path, with method color-coding (GET=green, POST=blue, PUT=yellow, DELETE=red, PATCH=magenta).

#### `r2e dev` — Development server

Wraps `cargo watch` with R2E-specific defaults:
- Watches `src/`, `application.yaml`, `application-dev.yaml`, `migrations/`
- Sets `R2E_PROFILE=dev` environment variable
- Prints discovered routes before starting the watch loop
- `--open` flag opens `http://localhost:8080` in the browser after a 5s delay

Requires `cargo-watch` to be installed (`cargo install cargo-watch`).

#### `r2e add <extension>` — Extension management

Adds an R2E sub-crate dependency to `Cargo.toml`. Known extensions: `security`, `data`, `openapi`, `events`, `scheduler`, `cache`, `rate-limit`, `utils`, `prometheus`, `test`.

#### Template system (`commands/templates/`)

Shared helpers in `templates/mod.rs`:
- `to_snake_case("UserController")` → `"user_controller"`
- `to_pascal_case("user_service")` → `"UserService"`
- `pluralize("user")` → `"users"`, `pluralize("category")` → `"categories"`
- `render(template, &[("key", "value")])` — `{{key}}` substitution

## Language & Documentation

The project's plan (`plan.md`) and step-by-step docs (`docs/steps/`) are written in French. Code, comments, and API surfaces are in English.
