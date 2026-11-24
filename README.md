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
- **Event bus** — Typed in-process pub/sub (`LocalEventBus`) with `#[consumer]` for declarative event handlers; distributed backends for Apache Iggy, Kafka, Pulsar and RabbitMQ (`events-iggy`, `events-kafka`, `events-pulsar`, `events-rabbitmq` features)
- **Scheduling** — `#[scheduled(every = 30)]` and `#[scheduled(cron = "0 */5 * * * *")]` for background tasks
- **Managed task pool** — `Executor` plugin (`PoolExecutor`), `#[async_exec]` methods returning a `JobHandle`, `#[derive(BackgroundService)]` workers, bounded concurrency and graceful drain
- **Feature modules** — `#[module]` closed subgraphs with imports/exports, path prefixes and bundled plugins, registered with `register_module`
- **Canonical entry point** — the `App` trait, booted by `r2e::app_main!`, `r2e::launch!` and `TestApp::boot` alike
- **Managed resources** — `#[managed]` for automatic transaction lifecycle (begin/commit/rollback)
- **Database ergonomics** — core `Pageable`/`Page` plus cancellation-safe managed SQLx and Diesel transactions
- **Validation** — Automatic validation via `garde` crate — just derive `Validate` and use `Json<T>`
- **OpenAPI** — Auto-generated OpenAPI 3.1.0 spec with interactive docs UI at `/docs`
- **Prometheus metrics** — Request metrics with configurable namespace and path exclusions
- **OpenTelemetry** — Distributed tracing and context propagation via OTLP exporter
- **gRPC** — Tonic-based gRPC server on a separate port or multiplexed on the HTTP port, with reflection and grpc-web
- **MCP server** — `#[tool]`, `#[resource]` and `#[prompt]` members in `#[routes]`, served over streamable HTTP by the `McpServer` plugin, with shared guards and OAuth 2.1 resource-server auth (`mcp` feature)
- **Multi-tenancy** — `Tenant<T>` / `Tenanted<T>` per-tenant beans, `Tenancy` and `PerTenant` plugins, per-tenant SQLx/Diesel pools (`tenant` feature)
- **OpenFGA** — schema-first fine-grained authorization: `model!` over a `.fga` file, typed `FgaCheck` guards, store lifecycle via the `OpenFga` plugin (`openfga` feature)
- **HTTP/3 (QUIC)** — `serve_h3` and raw QUIC endpoints (`quic` feature)
- **Sharded serving** — `server.workers: per-core` runs one runtime per core on `SO_REUSEPORT` sockets, with per-worker services and a control plane
- **Embedded OIDC** — Local RS256 issuer with Authorization Code + PKCE, client credentials, and an optional development password grant
- **Configuration** — YAML + env var overlay with typed `ConfigProperties` sections
- **SSE & WebSocket** — Built-in `SseBroadcaster` and `WsRooms` for real-time communication
- **Multipart** — Multipart form/file upload support
- **Static file serving** — Embed frontend assets in the binary with SPA fallback via `EmbeddedFrontend` plugin
- **Testing** — `TestApp`, `TestJwt`, and `r2e test --sonarqube` coverage for integration tests
- **Dev services** — Testcontainers-backed `DevPostgres`, `DevRedis`, `DevKeycloak` and generic `DevService` for tests (`r2e-devservices`)
- **CLI** — `r2e new`, `r2e add`, `r2e dev`, `r2e generate`, `r2e test` for scaffolding and workflows

## Quick start

Add R2E to your `Cargo.toml`:

```toml
[dependencies]
r2e = { version = "0.15", features = ["full"] }
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

    app.plugin(Health)                // GET /health
        .plugin(Cors::permissive())
        .plugin(HttpTrace::new())     // one span + one summary line per request
        .build_state()                // no type args; async — resolves the bean graph
        .await
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

### Canonical entry point: the `App` trait

For real apps, prefer declaring the wiring once via the `App` trait
(`setup` builds a long-lived environment, `build` assembles the `AppBuilder`)
and launching it with `r2e::app_main!(MyApp)` (or `r2e::launch!(MyApp)` / the
`r2e::launch::<MyApp>()` function). This single assembly path is shared by
production, dev-reload, and tests — tests boot the same type via
`TestApp::boot::<MyApp>()` or `#[r2e::test(app = MyApp)]`, with
`override_bean` / `override_config` / `override_config_value` for test doubles.

```rust
use r2e::prelude::*;

struct MyApp;

impl App for MyApp {
    type Env = ();
    async fn setup() -> Result<Self::Env, BootError> { Ok(()) }
    async fn build(b: AppBuilder, _env: Self::Env) -> Result<impl BootableApp, BootError> {
        Ok(b.register::<UserService>()
            .plugin(Health)
            .try_build_state()
            .await?
            .register_controller::<UserController>())
    }
}

r2e::app_main!(MyApp);
```

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

impl SelfBuilt for TenantGuard {}        // no bean deps: the expression is the guard

impl<I: Identity> Guard<I> for TenantGuard {
    fn check(&self, ctx: &GuardContext<'_, I>) -> impl Future<Output = Result<(), Rejection>> + Send {
        async move {
            match ctx.identity_claims() {
                Some(claims) if claims["tenant_id"].is_string() => Ok(()),
                _ => Err(Rejection::forbidden("Missing tenant")),   // rendered by the route's error envelope
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

// Provide the bus once: AppBuilder::new().provide(LocalEventBus::new())

// Emit events from services (`emit` resolves to a Result)
self.event_bus.emit(UserCreatedEvent { user_id: 1, name: "Alice".into() }).await?;

// Declarative consumer
#[controller]
pub struct UserEventConsumer {
    #[inject] event_bus: LocalEventBus,
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

    #[scheduled(every = 60, initial_delay = 10)]  // first run after 10s
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
    .plugin(OpenApiPlugin::new(
        OpenApiConfig::new("My API", "1.0.0")
            .with_description("API description")
            .with_docs_ui(true),  // serves interactive UI at /docs
    ))
    .build_state()
    .await
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
            .plugin(Health)
            .build_state()
            .await
            .register_controller::<UserController>(),
    );

    // Unauthenticated request
    app.get("/users").send().await.assert_unauthorized();

    // Authenticated request
    let token = jwt.token("user-1", &["user"]);
    let resp = app.get("/users").bearer(&token).send().await;
    resp.assert_ok();
    let users: Vec<User> = resp.json();
    assert_eq!(users.len(), 2);

    // Role-based access
    let admin_token = jwt.token("admin-1", &["admin"]);
    app.get("/admin/users").bearer(&admin_token).send().await.assert_ok();

    let user_token = jwt.token("user-1", &["user"]);
    app.get("/admin/users").bearer(&user_token).send().await.assert_forbidden();
}
```

## Plugins

R2E ships with built-in plugins. There is one plugin kind and one install call — `.plugin(..)`, always **before** `build_state()`:

| Plugin | Description |
|--------|-------------|
| `Health` | `GET /health` returning 200 "OK" |
| `Cors::new()` | CORS from `cors.*` (`allowed-origins` incl. `scheme://host:*`, methods, headers, credentials, `max-age`), `.default_origin(..)` below the file; `Cors::permissive()` for `*`, `Cors::custom(layer)` for your own `CorsLayer` |
| `Tracing` | Installs the log **subscriber** (`tracing_subscriber::fmt`). Redundant under `r2e::launch` / `#[r2e::main]`, which install it already |
| `HttpTrace::new()` | Per-request span + summary line, request ids, path exclusions (`trace.*`) |
| `NormalizePath` | Trailing-slash normalization (install order irrelevant) |
| `DevReload` | Dev-mode `/__r2e_dev/*` endpoints |
| `RequestIdPlugin` | X-Request-Id propagation |
| `SecureHeaders` | Security headers (X-Content-Type-Options, etc.) |
| `OpenApiPlugin` | OpenAPI spec + docs UI |
| `Prometheus` | Prometheus metrics at `/metrics` |
| `Observability` | OpenTelemetry distributed tracing (OTLP exporter) |
| `GrpcServer` | gRPC server on a dedicated port |
| `OidcServer` | Embedded issuer (`/oauth/authorize`, `/oauth/token`, JWKS endpoints) |
| `AdvancedHealth` | Liveness/readiness probes with pluggable health indicators (via `Health::builder()`) |
| `EmbeddedFrontend` | Embedded static file serving with SPA fallback |
| `Executor` | Managed task pool (`PoolExecutor` bean, `executor.*` config) behind `#[async_exec]`, `#[derive(BackgroundService)]` and `Scheduler`; drains on shutdown |
| `Scheduler` | Background task scheduling — requires `Executor`; ticks run on its pool |
| `MetricsFacade` | HTTP metrics through the `metrics` crate facade (`metrics-facade` feature), when the app already runs a `metrics` exporter |
| `McpServer` | MCP server (streamable HTTP) for `#[tool]` / `#[resource]` / `#[prompt]` services, `mcp.*` config |
| `Tenancy` / `PerTenant` | Tenant resolution (`TenantRouter`) and per-tenant copies of a resource (`Tenanted<T>`), `tenancy.*` config |
| `OpenFga` | OpenFGA store lifecycle at boot (create/verify store, apply the compiled-in model) |
| `WorkerHealth` | Readiness indicator for sharded serving: `/health/ready` turns 503 when the first worker starts draining |

## Crate map

```
r2e                  Facade crate — re-exports everything, feature-gated
r2e-rt               Async-runtime facade — the only crate naming tokio
r2e-tracing          Process-global tracing subscriber: options, one-shot install, boot-log buffer
r2e-http             HTTP abstraction layer — sole owner of the axum dependency; QUIC/HTTP3
r2e-core             Runtime: AppBuilder, Controller, guards, interceptors, config, plugins, SSE, WS
r2e-macros           Proc macros: #[controller], #[routes], #[bean], #[module], …
r2e-security         JWT/OIDC: AuthenticatedUser, JwtValidator, JWKS cache
r2e-events           In-process typed EventBus (LocalEventBus) with pub/sub and request-reply
r2e-events-iggy      Distributed EventBus backend: Apache Iggy
r2e-events-kafka     Distributed EventBus backend: Apache Kafka
r2e-events-pulsar    Distributed EventBus backend: Apache Pulsar
r2e-events-rabbitmq  Distributed EventBus backend: RabbitMQ
r2e-scheduler        Background task scheduling (interval, cron) on the Executor pool
r2e-executor         Managed task pool: PoolExecutor, #[async_exec], #[derive(BackgroundService)]
r2e-data-sqlx        Managed SQLx Tx + SqlxDataSource (sqlite/postgres/mysql)
r2e-data-diesel      Managed Diesel Tx + DieselDataSource (sqlite/postgres/mysql)
r2e-cache            TTL cache with pluggable backends
r2e-rate-limit       Token-bucket rate limiting with pluggable backends
r2e-openapi          OpenAPI 3.1.0 spec generation + docs UI
r2e-prometheus       HTTP metrics (Prometheus, layer-only, or metrics facade)
r2e-observability    OpenTelemetry distributed tracing + context propagation (OTLP)
r2e-grpc             gRPC server support via Tonic, separate port or multiplexed with HTTP
r2e-grpc-build       build.rs helper that compiles .proto files for r2e-grpc services
r2e-mcp              MCP server: #[tool]/#[resource]/#[prompt], McpServer plugin, OAuth 2.1 auth
r2e-oidc             Embedded issuer: JWT, Authorization Code + PKCE, client credentials
r2e-openfga          OpenFGA fine-grained authorization (Zanzibar-style)
r2e-openfga-model    OpenFGA DSL parser and model validation (schema 1.1)
r2e-openfga-macros   model! macro: typed API generated from a .fga file
r2e-tenant           Multi-tenant bean routing: Tenant<T>, Tenanted<T>, Tenancy/PerTenant plugins
r2e-utils            Built-in interceptors: Logged, Timed, Cache, CacheInvalidate
r2e-test             TestApp, TestJwt, TestSession, WsTestClient for integration testing
r2e-devservices      Testcontainers dev services: DevPostgres, DevRedis, DevKeycloak, DevService
r2e-devtools         Subsecond hot-reload (dev-reload feature only, not for production)
r2e-static           Embedded static file serving with SPA support (wraps rust_embed)
r2e-cli              CLI scaffolding tool (not on crates.io: cargo install --path r2e-cli)
```

For a detailed file-by-file breakdown of every crate, see [REPO_MAP.md](REPO_MAP.md).

## Examples

The `examples/` directory contains runnable demo apps:

| Example | Description |
|---------|-------------|
| [`example-app`](examples/example-app/README.md) | Kitchen-sink demo exercising most features (modules, security, events, scheduling, SSE/WS, OpenFGA, proxy) |
| [`example-executor`](examples/example-executor/README.md) | `#[async_exec]` jobs and a `#[derive(BackgroundService)]` worker on the `Executor` pool |
| [`example-grpc`](examples/example-grpc/README.md) | HTTP + gRPC on separate ports, reflection, grpc-web, gRPC guards and interceptors |
| [`example-mcp`](examples/example-mcp/README.md) | One bean behind an HTTP controller and two MCP services (tools, resources, prompts, opt-in groups) |
| [`example-mcp-oauth`](examples/example-mcp-oauth/README.md) | MCP server behind an OAuth 2.1 resource server, ready for Claude / ChatGPT connectors |
| [`example-microservice`](examples/example-microservice/README.md) | Two services with inter-service HTTP calls and shared types |
| [`example-multi-tenant`](examples/example-multi-tenant/README.md) | Row-level tenant isolation with a custom `TenantGuard` |
| [`example-multi-tenant-db`](examples/example-multi-tenant-db/README.md) | Database-per-tenant with the `tenant` feature (`Tenant<T>`, `Tenanted<T>`, `TenantTx`) |
| [`example-oidc`](examples/example-oidc/README.md) | Embedded issuer with password and client_credentials grants |
| [`example-openfga`](examples/example-openfga/README.md) | Schema-first OpenFGA authorization with typed `FgaCheck` guards |
| [`example-postgres`](examples/example-postgres/README.md) | CRUD with PostgreSQL, SQLx migrations, pagination, validation |
| [`example-sharded-bench`](examples/example-sharded-bench/README.md) | Benchmark target for sharded serving (`server.workers`) |
| [`example-websocket-chat`](examples/example-websocket-chat/README.md) | Real-time chat using `WsRooms` and event-driven persistence |
| [`example-worker-udp`](examples/example-worker-udp/README.md) | Per-worker services under `server.workers: per-core`: UDP echo, mailboxes, worker health |

```bash
cargo run -p example-app --bin example-app   # port 3001
cargo run -p example-postgres                # requires a running PostgreSQL
cargo run -p example-grpc                    # HTTP :3000, gRPC :50051
```

## Building

```bash
cargo build --workspace        # build all crates
cargo check --workspace        # type-check (faster)
cargo test --workspace         # run all tests
cargo run -p example-app --bin example-app   # run the demo app on port 3001
```

## Versioning

R2E is published on crates.io; depend on the facade `r2e` (plus `r2e-test` in
`[dev-dependencies]`). **Every crate is versioned independently** and follows
semver with pre-1.0 semantics: a breaking change in a crate bumps **that
crate's** minor version and, because they re-export its types, the minor of
every crate depending on it — `r2e` included. A patch of one crate is picked up
by a plain `cargo update`. Each crate has its own `CHANGELOG.md`
(`r2e/CHANGELOG.md` is the one to read for the facade) and its own tags,
`<crate>-vX.Y.Z`; `r2e-cli` is versioned with `r2e`.

Up to 0.6.0 the whole workspace moved in lockstep under a single `vX.Y.Z` tag
and the root [`CHANGELOG.md`](CHANGELOG.md), kept as frozen history: a legacy
`vX.Y.Z` tag is release `X.Y.Z` of every crate. Older still, tags up to
`v0.2.163` were a pure release counter detached from the manifest —
`v0.2.132`–`v0.2.163` actually contain workspace version `0.3.0`, and the 0.3
plugin-API rework ships from `v0.2.140` onward (see
[`docs/migration/plugin-api.md`](docs/migration/plugin-api.md)).

Migration guides for the breaking releases live under `docs/migration/`:

- 0.3: the plugin API — post-state `.with()` becomes one `Plugin` trait:
  [`plugin-api.md`](docs/migration/plugin-api.md).
- 0.4 → 0.5 reworks error handling around `Rejection` and error envelopes
  (task #1072): [`error-projection.md`](docs/migration/error-projection.md).
- 0.5 → 0.6 documents OpenAPI bodies only through `RequestBodySchema` /
  `ResponseBodySchema` (no name-based `Json<T>` detection):
  [`response-schema.md`](docs/migration/response-schema.md).
- r2e-mcp 0.6 → 0.7: the MCP global registry becomes a dynamic `McpRegistry`
  bean: [`mcp-registry.md`](docs/migration/mcp-registry.md).
- r2e-core 0.6 → 0.7 shrinks `Rejection` to 56 bytes; `details`/`headers`
  become accessors (task #1086):
  [`rejection-layout.md`](docs/migration/rejection-layout.md).
- r2e-core 0.7 → 0.8: `Routes` replaces `axum::Router`:
  [`routes-newtype.md`](docs/migration/routes-newtype.md).
- r2e-core 0.9: one tracing path through the builder:
  [`tracing-single-path.md`](docs/migration/tracing-single-path.md)
  (supersedes [`observability-tracing.md`](docs/migration/observability-tracing.md)).
- r2e-core 0.9 → 0.10: HTTP layer bands (`LayerSlot`):
  [`layer-slots.md`](docs/migration/layer-slots.md).
- r2e-mcp 0.9 → 0.10: MCP mount stage and identity contract:
  [`mcp-identity.md`](docs/migration/mcp-identity.md).
- r2e-grpc 0.9 → 0.10: gRPC identity rides the HTTP extractor contract:
  [`grpc-identity.md`](docs/migration/grpc-identity.md).
- r2e-core 0.11 → 0.12: generic DI builder bounds (provision index):
  [`di-provision-index.md`](docs/migration/di-provision-index.md).

## For AI agents

If you are an AI agent or LLM, start at [llm.txt](llm.txt) — golden rules plus a routing table that maps your task to one topic file under [llm/](llm/) (read only those). [llm-full.txt](llm-full.txt) is the same reference as a single document, for tools that ingest one file.

Inside a project, `r2e docs --llm --export` writes the reference matching the installed R2E version into `docs/r2e/` (`r2e new` does it for you, `r2e doctor` warns when the copy is stale); `r2e docs --llm [<topic>] [--full]` prints it.

## License

Apache-2.0
