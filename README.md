# R2E — Rust Enterprise Edition

An ergonomic enterprise framework for Rust, inspired by Quarkus. Declarative controllers, compile-time dependency injection, JWT/OIDC security, and zero runtime reflection. Built on [Axum](https://github.com/tokio-rs/axum) via the `r2e-http` abstraction layer.

```rust
#[controller(path = "/users")]
pub struct UserController {
    #[inject]           user_service: UserService,
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

    #[post("/")]
    #[roles("admin")]
    #[intercept(CacheInvalidate::group("users"))]
    async fn create(&self, Json(body): Json<CreateUserRequest>) -> Json<User> {
        Json(self.user_service.create(body.name, body.email).await)
    }
}
```

## Features

- **Declarative controllers** — `#[controller]` + `#[routes]` generate Axum handlers with zero boilerplate
- **Compile-time DI** — `#[inject]` resolves beans from the graph by type, `#[inject(identity)]`/`#[inject(request)]` for request-scoped fields, `#[config("key")]` for configuration. The application state is an inferred HList of everything you `.provide(...)`/`.register::<T>()` — you never hand-write a state struct.
- **JWT/OIDC security** — `AuthenticatedUser` extractor with JWKS caching, role-based access via `#[roles("admin")]`
- **Guards** — Pre-auth and post-auth guards (`#[guard(...)]`, `#[pre_guard(...)]`) for custom authorization logic
- **Interceptors** — AOP-style `#[intercept(...)]` for logging, timing, caching, and custom cross-cutting concerns
- **Rate limiting** — Token-bucket rate limiting per user, per IP, or global via `RateLimit::per_user(5, 60)`
- **Event bus** — Typed in-process pub/sub with `#[consumer]` for declarative event handlers
- **Scheduling** — `#[scheduled(every = 30)]` and `#[scheduled(cron = "0 */5 * * * *")]` for background tasks
- **Managed resources** — `#[managed]` for automatic transaction lifecycle (begin/commit/rollback)
- **Database ergonomics** — core `Pageable`/`Page` plus cancellation-safe managed SQLx and Diesel transactions
- **Validation** — Automatic validation via `garde` crate — just derive `Validate` and use `Json<T>`
- **OpenAPI** — Auto-generated OpenAPI 3.1.0 spec with interactive docs UI at `/docs`
- **Prometheus metrics** — Request metrics with configurable namespace and path exclusions
- **OpenTelemetry** — Distributed tracing and context propagation via OTLP exporter
- **gRPC** — Tonic-based gRPC server support, multiplexed alongside HTTP on separate ports
- **Embedded OIDC** — Built-in OIDC server issuing JWTs (password + client_credentials grants) without an external IdP
- **Configuration** — YAML + env var overlay with typed `ConfigProperties` sections
- **SSE & WebSocket** — Built-in `SseBroadcaster` and `WsRooms` for real-time communication
- **Multipart** — Multipart form/file upload support
- **Static file serving** — Embed frontend assets in the binary with SPA fallback via `EmbeddedFrontend` plugin
- **Testing** — `TestApp` HTTP client wrapper and `TestJwt` token generator for integration tests
- **CLI** — `r2e new`, `r2e add`, `r2e dev`, `r2e generate` for scaffolding

## Quick start

Add R2E to your `Cargo.toml`:

```toml
[dependencies]
r2e = { version = "0.1", features = ["full"] }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

Define a service. R2E has no hand-written state struct — every bean you register
becomes part of an inferred HList state:

```rust
use r2e::prelude::*;

#[derive(Clone)]
pub struct UserService { /* ... */ }

#[bean]
impl UserService {
    pub fn new() -> Self {
        Self { /* ... */ }
    }
}
```

Define a controller (no `state = ...`; `#[inject]` fields resolve from the bean
graph by type):

```rust
use r2e::prelude::*;

#[controller(path = "/users")]
pub struct UserController {
    #[inject]
    user_service: UserService,
}

#[routes]
impl UserController {
    #[get("/")]
    async fn list(&self) -> Json<Vec<User>> {
        Json(self.user_service.list().await)
    }

    #[get("/{id}")]
    async fn get_by_id(&self, Path(id): Path<u64>) -> Result<Json<User>, HttpError> {
        self.user_service.get_by_id(id).await
            .map(Json)
            .ok_or_else(|| HttpError::NotFound("User not found".into()))
    }

    #[post("/")]
    async fn create(&self, Json(body): Json<CreateUserRequest>) -> Json<User> {
        Json(self.user_service.create(body.name, body.email).await)
    }
}
```

Wire it up in `main.rs`:

```rust
#[r2e::main]
async fn main() {
    let app = AppBuilder::new()
        .load_config::<()>()          // typed config from YAML+env — the sole config registration point
        .register::<UserService>();   // register a #[bean]/#[producer]/AsyncBean type

    app.build_state()                 // no type args; async — resolves the bean graph
        .await
        .with(Health)                 // GET /health
        .with(Cors::permissive())
        .with(Tracing)
        .with(ErrorHandling)
        .register_controller::<UserController>()   // after build_state
        .serve("0.0.0.0:3000")
        .await
        .unwrap();
}
```

`build_state()` takes **no type arguments** and is `async` — it resolves the
bean dependency graph (topological sort) and materializes the inferred HList
state. Use `try_build_state()` for the non-panicking `Result` variant. Provide
pre-built values with `.provide(value)` and registered types with
`.register::<T>()`; a `#[controller]`'s `#[inject]` fields are then satisfied by
type — a missing bean is a **compile error naming the type**.

Apps with more than ~127 provisions need `#![recursion_limit = "512"]` at the
crate root; `r2e doctor` warns as the bean count approaches that threshold.

## Injection scopes

All injection is resolved at compile time — no runtime reflection, no trait objects.

| Attribute | Scope | Description |
|-----------|-------|-------------|
| `#[inject]` | App | Resolved from the bean graph by type. Type must be `Clone + Send + Sync`. |
| `#[inject(identity)]` | Request | Extracted via `FromRequestPartsVia` (e.g. `AuthenticatedUser`); plain axum `FromRequestParts` extractors bridge automatically. |
| `#[config("key")]` | App | Resolved from `R2eConfig`. Supports `String`, `i64`, `f64`, `bool`, `Option<T>`. |

`#[inject(identity)]` can be placed on struct fields (all endpoints require auth) or on handler parameters (mixed public/protected endpoints):

```rust
// Mixed controller — some endpoints public, some protected
#[controller(path = "/api")]
pub struct ApiController {
    #[inject] service: MyService,
}

#[routes]
impl ApiController {
    #[get("/public")]
    async fn public_data(&self) -> Json<Data> { /* ... */ }

    #[get("/me")]
    async fn me(&self, #[inject(identity)] user: AuthenticatedUser) -> Json<User> {
        Json(user)
    }
}
```

## Security

```rust
use r2e::r2e_security::{JwtClaimsValidator, SecurityConfig};

// Static key (testing/simple setups)
let config = SecurityConfig::new("jwks-url", "issuer", "audience");
let validator = JwtClaimsValidator::new_with_static_key(decoding_key, config);

// JWKS endpoint (production)
let validator = JwtClaimsValidator::new(config); // fetches keys from JWKS URL
```

Role-based access control:

```rust
#[get("/admin")]
#[roles("admin")]
async fn admin_only(&self) -> Json<&'static str> {
    Json("secret")
}
```

## Guards

Post-auth guards (run after JWT validation):

```rust
use r2e::r2e_rate_limit::RateLimit;

#[post("/")]
#[guard(RateLimit::per_user(5, 60))]  // 5 requests per 60 seconds per user
async fn create(&self, body: Json<Request>) -> Json<Response> { /* ... */ }
```

Pre-auth guards (run before JWT validation):

```rust
#[get("/")]
#[pre_guard(RateLimit::global(100, 60))]  // 100 requests per 60 seconds total
#[pre_guard(RateLimit::per_ip(10, 60))]   // 10 requests per 60 seconds per IP
async fn list(&self) -> Json<Vec<Item>> { /* ... */ }
```

Custom guards:

```rust
struct TenantGuard;

impl<S: Send + Sync, I: Identity> Guard<S, I> for TenantGuard {
    fn check(&self, state: &S, ctx: &GuardContext<'_, I>) -> impl Future<Output = Result<(), Response>> + Send {
        async move {
            match ctx.identity_claims() {
                Some(claims) if claims["tenant_id"].is_string() => Ok(()),
                _ => Err(HttpError::Forbidden("Missing tenant".into()).into_response()),
            }
        }
    }
}

#[get("/")]
#[guard(TenantGuard)]
async fn tenant_data(&self) -> Json<Data> { /* ... */ }
```

## Interceptors

```rust
#[routes]
#[intercept(Logged::info())]                    // log all methods in this controller
impl UserController {
    #[get("/")]
    #[intercept(Timed::threshold(50))]          // log if >50ms
    #[intercept(Cache::ttl(30).group("users"))] // cache for 30s
    async fn list(&self) -> Json<Vec<User>> { /* ... */ }

    #[post("/")]
    #[intercept(CacheInvalidate::group("users"))] // clear cache on write
    async fn create(&self, body: Json<Request>) -> Json<User> { /* ... */ }
}
```

Custom interceptors:

Interceptors are generic over the state `S`. Stay `S: Send + Sync` if you never
touch state, or add `S: BeanLookup` to read a bean by type via `ctx.state.bean::<T>()`:

```rust
pub struct AuditLog;

impl<R: Send, S: Send + Sync> Interceptor<R, S> for AuditLog {
    fn around<F, Fut>(&self, ctx: InterceptorContext<'_, S>, next: F) -> impl Future<Output = R> + Send
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

## Events

```rust
#[derive(Debug, Clone)]
pub struct UserCreatedEvent {
    pub user_id: u64,
    pub name: String,
}

// Emit events from services
self.event_bus.emit(UserCreatedEvent { user_id: 1, name: "Alice".into() }).await;

// Declarative consumer
#[controller]
pub struct UserEventConsumer {
    #[inject] event_bus: EventBus,
}

#[routes]
impl UserEventConsumer {
    #[consumer(bus = "event_bus")]
    async fn on_user_created(&self, event: Arc<UserCreatedEvent>) {
        tracing::info!(user_id = event.user_id, "User created");
    }
}
```

## Scheduling

```rust
#[controller]
pub struct ScheduledJobs {
    #[inject] user_service: UserService,
}

#[routes]
impl ScheduledJobs {
    #[scheduled(every = 30)]                      // every 30 seconds
    async fn count_users(&self) {
        let count = self.user_service.count().await;
        tracing::info!(count, "Scheduled user count");
    }

    #[scheduled(cron = "0 0 * * * *")]            // every hour
    async fn hourly_cleanup(&self) { /* ... */ }

    #[scheduled(every = 60, delay = 10)]          // first run after 10s
    async fn delayed_task(&self) { /* ... */ }
}
```

Register the scheduler plugin **before** `build_state()`:

```rust
AppBuilder::new()
    .plugin(Executor)    // required by Scheduler (ticks run on the pool)
    .plugin(Scheduler)
    .register::<UserService>()
    .build_state()
    .await
    .register_controller::<ScheduledJobs>()
    .serve("0.0.0.0:3000")
    .await
    .unwrap();
```

## Managed resources

```rust
#[post("/")]
async fn create(
    &self,
    body: Json<CreateUserRequest>,
    #[managed] tx: &mut Tx<'_, Sqlite>,  // auto begin/commit/rollback
) -> Result<Json<User>, HttpError> {
    sqlx::query("INSERT INTO users (name, email) VALUES (?, ?)")
        .bind(&body.name)
        .bind(&body.email)
        .execute(tx.as_mut())
        .await?;
    Ok(Json(user))
}
```

`Tx` fetches the connection pool from the bean graph by type — just
`.provide(pool)` your `sqlx::Pool<DB>` and `#[managed] tx: &mut Tx<'_, DB>` works.
There is no `HasPool` trait to implement.

## Configuration

YAML-based with environment variable overlay:

```yaml
# application.yaml
app:
  greeting: "Hello"
  max-retries: 3
```

```rust
let config = R2eConfig::load().unwrap(); // loads application.yaml + .env + env vars

// Access in controllers via #[config("app.greeting")]
```

## OpenAPI

```rust
use r2e::r2e_openapi::{OpenApiConfig, OpenApiPlugin};

AppBuilder::new()
    .register::<UserService>()
    .build_state()
    .await
    .with(OpenApiPlugin::new(
        OpenApiConfig::new("My API", "1.0.0")
            .with_description("API description")
            .with_docs_ui(true),  // serves interactive UI at /docs
    ))
    .register_controller::<UserController>()
    .serve("0.0.0.0:3000")
    .await
    .unwrap();

// GET /openapi.json  — OpenAPI 3.1.0 spec
// GET /docs          — interactive API docs
```

## Testing

```rust
use r2e_test::{TestApp, TestJwt};

#[r2e::test]
async fn test_list_users() {
    let jwt = TestJwt::new();
    let app = TestApp::from_builder(
        AppBuilder::new()
            .provide(std::sync::Arc::new(jwt.claims_validator()))  // Arc<JwtClaimsValidator> bean
            .register::<UserService>()
            .build_state()
            .await
            .with(Health)
            .with(ErrorHandling)
            .register_controller::<UserController>(),
    );

    // Unauthenticated request
    app.get("/users").await.assert_unauthorized();

    // Authenticated request
    let token = jwt.token("user-1", &["user"]);
    let resp = app.get_authenticated("/users", &token).await.assert_ok();
    let users: Vec<User> = resp.json();
    assert_eq!(users.len(), 2);

    // Role-based access
    let admin_token = jwt.token("admin-1", &["admin"]);
    app.get_authenticated("/admin/users", &admin_token).await.assert_ok();

    let user_token = jwt.token("user-1", &["user"]);
    app.get_authenticated("/admin/users", &user_token).await.assert_forbidden();
}
```

## Plugins

R2E ships with built-in plugins that install with a single `.with(...)` call:

| Plugin | Description |
|--------|-------------|
| `Health` | `GET /health` returning 200 "OK" |
| `Cors::permissive()` | Permissive CORS headers (or `Cors::new(layer)` for custom) |
| `Tracing` | Request tracing via `tracing` + `tower-http` |
| `ErrorHandling` | Catches panics, returns JSON 500 |
| `NormalizePath` | Trailing-slash normalization (install last) |
| `DevReload` | Dev-mode `/__r2e_dev/*` endpoints |
| `RequestIdPlugin` | X-Request-Id propagation |
| `SecureHeaders` | Security headers (X-Content-Type-Options, etc.) |
| `OpenApiPlugin` | OpenAPI spec + docs UI |
| `Prometheus` | Prometheus metrics at `/metrics` |
| `Observability` | OpenTelemetry distributed tracing (OTLP exporter) |
| `GrpcServer` | gRPC server on a dedicated port (install via `.plugin()` before `build_state()`) |
| `OidcServer` | Embedded OIDC server (`/oauth/token`, JWKS endpoints) |
| `AdvancedHealth` | Liveness/readiness probes with pluggable health indicators (via `Health::builder()`) |
| `EmbeddedFrontend` | Embedded static file serving with SPA fallback (install last) |
| `Scheduler` | Background task scheduling — requires `Executor`; ticks run on its pool (install via `.plugin()` before `build_state()`) |

## Crate map

```
r2e               Facade crate — re-exports everything, feature-gated
r2e-http          HTTP abstraction layer — sole owner of the axum dependency
r2e-core          Runtime: AppBuilder, Controller, guards, interceptors, config, plugins, SSE, WS
r2e-macros        Proc macros: #[controller], #[routes], #[bean]
r2e-security      JWT/OIDC: AuthenticatedUser, JwtValidator, JWKS cache
r2e-events        In-process typed EventBus with pub/sub
r2e-scheduler     Background task scheduling (interval, cron)
r2e-data-sqlx     Managed SQLx Tx (sqlite/postgres/mysql)
r2e-data-diesel   Managed Diesel Tx (sqlite/postgres/mysql)
r2e-cache         TTL cache with pluggable backends
r2e-rate-limit    Token-bucket rate limiting with pluggable backends
r2e-openapi       OpenAPI 3.1.0 spec generation + docs UI
r2e-prometheus    Prometheus metrics middleware
r2e-observability OpenTelemetry distributed tracing + context propagation (OTLP)
r2e-grpc          gRPC server support via Tonic, multiplexed with HTTP
r2e-oidc          Embedded OIDC server: JWT issuance, password + client_credentials grants
r2e-openfga       OpenFGA fine-grained authorization (Zanzibar-style)
r2e-utils         Built-in interceptors: Logged, Timed, Cache, CacheInvalidate
r2e-test          TestApp, TestJwt for integration testing
r2e-devtools      Subsecond hot-reload (dev-reload feature only, not for production)
r2e-static        Embedded static file serving with SPA support (wraps rust_embed)
r2e-cli           CLI scaffolding tool
```

For a detailed file-by-file breakdown of every crate, see [REPO_MAP.md](REPO_MAP.md).

## Examples

The `examples/` directory contains runnable demo apps:

| Example | Description |
|---------|-------------|
| `example-app` | Kitchen-sink demo exercising all features |
| `example-postgres` | CRUD with PostgreSQL, SQLx migrations, OpenAPI docs |
| `example-multi-tenant` | JWT-based tenant isolation with custom `TenantGuard` |
| `example-websocket-chat` | Real-time chat using `WsRooms` and event-driven persistence |
| `example-grpc` | HTTP + gRPC multiplexing on separate ports |
| `example-microservice` | Two services with inter-service HTTP calls and shared types |
| `example-oidc` | Embedded OIDC server with password and client_credentials grants |

```bash
cargo run -p example-app             # port 3000
cargo run -p example-postgres        # requires a running PostgreSQL
cargo run -p example-grpc            # HTTP :3000, gRPC :50051
```

## Building

```bash
cargo build --workspace        # build all crates
cargo check --workspace        # type-check (faster)
cargo test --workspace         # run all tests
cargo run -p example-app       # run the demo app on port 3001
```

## For AI agents

If you are an AI agent or LLM, read [llm.txt](llm.txt) for a comprehensive API reference.

## License

Apache-2.0
