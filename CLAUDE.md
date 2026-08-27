# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

AGENTS.md-aware agents are bound through `AGENTS.md`, which delegates to this
file. Keep cross-agent guidance changes visible from both `CLAUDE.md` and
`AGENTS.md`.

## Project Status

R2E is **not in production yet**. Breaking changes are always allowed — no need to gate them behind feature flags or maintain backward compatibility. Just mention breaking changes explicitly in plans so they are acknowledged.

## Build Commands

```bash
cargo build --workspace            # Build all crates
cargo check --workspace            # Check all crates (faster, no codegen)
cargo check -p r2e-core --features dev-reload   # dev-reload is off by default; check it explicitly
cargo run -p example-app           # Run the example app (serves on 0.0.0.0:3000)
cargo test --workspace             # Run tests
cargo build -p <crate-name>        # Build a specific crate
cargo expand -p example-app        # Expand macros (requires cargo-expand)
```

## Testing Conventions

**Tests live in `<crate>/tests/` directories, not inline.** Do NOT use `#[cfg(test)] mod tests { ... }` blocks inside source files.

- One test module per source module: `src/foo.rs` → `tests/<subsystem>/foo.rs`
- Use external imports (`use <crate_name>::...`) instead of `use super::*`
- Keep test-only helpers in the test file, not in the source
- If a test needs access to an internal item, add a `pub` accessor or `pub` + `#[doc(hidden)]` — do NOT use `#[cfg(test)] pub(crate)` visibility hacks

**Group test modules into one target per subsystem.** Cargo turns each
`tests/<name>/main.rs` into a target named `<name>`; the sibling files are
plain `mod`s. One binary per subsystem instead of one per file keeps link time
down and gives shared fixtures a home. Do NOT add new top-level `tests/*.rs`
files to a crate that already uses this layout — add a `mod` to the matching
target.

`r2e-core/tests/` is the reference layout:

```
support/mod.rs   # helpers shared across targets (not a target: no main.rs).
                 # Each main.rs pulls it in with
                 #   #[path = "../support/mod.rs"] mod support;
config/          # R2eConfig, ConfigProperties, sections, env overlay,
                 #   secrets, loading, startup validation
di/              # bean graph, async beans, producers, optional/lazy beans,
                 #   defaults, pinned overrides, lifecycle, modules
builder/         # AppBuilder: HList state, overrides, prepared, App trait
controller/      # request façade, core-only path, #[anonymous], injection
                 #   scopes, proxy/catch-all routing
decorators/      # DecoratorSpec, guards, interceptors
plugin/          # Provided/Deps/Late, deferred surface, config, lifecycle
http/            # extractors, errors, SSE/WS, managed resources, HTTP plugins
runtime/         # rt, sharded serving, socket options, tracing, dev-reload
```

Conventions inside a target:

- Fixtures used by more than one module go in `<target>/fixtures.rs`; keep
  single-use fixtures next to the test that needs them.
- Feature gates go on the `mod` declaration in `main.rs`
  (`#[cfg(feature = "ws")] mod ws;`), not as `#![cfg(...)]` inside the file.
- Tests that mutate `std::env` share a process now — take
  `crate::support::env_lock()` for the whole test, even when variable names
  don't overlap.

```bash
cargo test --workspace                    # all tests
cargo test -p r2e-core                    # single crate
cargo test -p r2e-core --test config      # one subsystem target
cargo test -p r2e-core --test config sections::   # one module within it
```

## Architecture

R2E is a **Quarkus-like ergonomic layer over Axum** for Rust. It provides declarative controllers with compile-time dependency injection, JWT/OIDC security, and zero runtime reflection.

### Workspace Crates

```
r2e             → Facade crate. Re-exports all subcrates behind feature flags. Users depend on this.
r2e-rt          → Async-runtime facade. The ONLY workspace crate that names tokio/tokio-util/tokio-stream directly, and it sits at the BOTTOM of the graph (below r2e-http). Owns spawn/spawn_ctl/spawn_blocking + JobHandle, sleep/timeout/interval, bind_tcp, shutdown_signal, CancelToken/CancelDropGuard, the `sync` re-exports (mpsc/oneshot/broadcast/watch/Mutex/RwLock/Notify/Semaphore/OnceCell), select!/pin!/join!/JoinSet, `stream`, TcpListener/TcpStream, the `io` module (AsyncRead/AsyncWrite + Ext traits), and RuntimeBuilder/Runtime/block_on. Re-exported as `r2e_core::rt` (hence `r2e::rt`).
r2e-macros      → Proc-macro crate. #[controller] + #[routes] generate Axum handlers.
r2e-http        → HTTP abstraction layer. Sole owner of the axum dependency; re-exports Router, extractors, responses, middleware, routing, WebSocket, multipart, and QUIC/HTTP3 types. Owns `IntoHttpResponse` (R2E's response contract) + the `impl_into_response!` bridge macro, `axum_compat` (the explicit raw-axum escape hatch), and the JSON codec façade `json` (`to_vec`/`from_slice`/`JsonError`, re-exported as `r2e_core::json`) with R2E's own `Json<T>` extractor/response + `JsonRejection`; codec selected by feature (`json-sonic` = sonic-rs, default serde_json). QUIC support (feature `quic`) provides HTTP/3 via h3+h3-quinn (bridged to axum Router) and raw QUIC streams via quinn.
r2e-core        → Runtime foundation. AppBuilder (load_config, with_config, build_state → HList state, serve_auto), Controller trait, ContextConstruct, PostConstruct, HttpError, Guard, Interceptor, R2eConfig, lifecycle hooks. Re-exports r2e-http as `http` module.
r2e-security    → JWT validation, JWKS cache, AuthenticatedUser extractor, RoleExtractor trait. Feature `grpc`: `JwtClaimsValidator` implements `r2e_grpc::JwtClaimsValidatorLike` (same bean authenticates HTTP + gRPC).
r2e-events      → In-process EventBus with typed pub/sub (emit/subscribe fan-out) plus point-to-point request-reply (request/respond). Shared backend utilities in `backend` module. Distributed backends live in `r2e-events/backends/`.
  backends/iggy     → Apache Iggy EventBus backend: persistent distributed event streaming.
  backends/kafka    → Apache Kafka EventBus backend: distributed event streaming via rdkafka.
  backends/pulsar   → Apache Pulsar EventBus backend: distributed event streaming.
  backends/rabbitmq → RabbitMQ (AMQP 0-9-1) EventBus backend: durable message queuing via lapin.
r2e-scheduler   → Background task scheduling (interval, cron, initial delay). CancelToken-based shutdown. All schedules are driven by a single driver task (min-heap of next-fire times), not one Tokio task per schedule. Requires the Executor plugin (`type Deps = (PoolExecutor,)`); each tick runs as a pool job (`executor.submit`) so ticks drain on shutdown, panics stay contained, and scheduled work is bounded by `executor.max-concurrent` and visible in `ExecutorMetrics`. Per-task overlap policy (`#[scheduled(overlap = "skip"|"concurrent")]` / `ScheduledTaskDef::with_overlap`, default skip) + skip predicate (`#[scheduled(skip_if = "method")]` / `ScheduledTaskDef::with_skip_if` — Quarkus skipExecutionIf; skips counted in `ScheduledJobInfo::skip_count`). Runtime control via `SchedulerHandle::{pause,resume,trigger_now}` + live `ScheduledJobInfo` stats. Optional dedicated pool + `scheduler.enabled` gate via `scheduler.*` config (`SchedulerConfig`, `CONFIG_PREFIX = "scheduler"`).
r2e-executor    → Managed task pool (PoolExecutor) + #[async_exec] + #[derive(BackgroundService)]. Bounded concurrency, graceful drain.
r2e-core        → Also owns Page/Pageable and the cancellation-safe ManagedResource lifecycle.
  r2e-data/backends/sqlx   → Managed SQLx transactions (SQLite/Postgres/MySQL). `SqlxDataSource<DB, Tag>` plugin: `datasource.*` config → connected `DbPool<DB, Tag>` bean, `migrate-at-start` runs the attached `sqlx::migrate!` inside `build_state()`, pool closed at shutdown; named datasources via `datasource_tag!`. `DataSourceHealth<DB, Tag>` (`Deps = (DbPool<DB, Tag>, HealthRegistry)`) contributes a `SELECT 1` health indicator (`.liveness_only()` to keep it out of readiness). Feature `tenant`: per-tenant pools (`TenantPools<DB>`, `PoolSource<DB>`) + `TenantTx<'_, DB>`.
  r2e-data/backends/diesel → Managed Diesel transactions (SQLite/Postgres/MySQL). `DieselDataSource<Conn, Tag>` mirrors `SqlxDataSource` (same `datasource.*` section, `embed_migrations!`, same `DataSourceHealth<Conn, Tag>`). Feature `tenant`: per-tenant r2d2 pools (`TenantPools<Conn>`, `PoolSource<Conn>`) + `TenantTx<Conn>`.
r2e-grpc        → Tonic-based gRPC server support, multiplexed alongside HTTP on separate ports. `include_protos!()` includes the modules generated by the build helper.
  build/            → r2e-grpc-build: build-script helper — compiles every proto/**/*.proto (tonic-prost-build), emits an aggregated per-package module + combined FileDescriptorSet into OUT_DIR (one-line build.rs, rerun-if-changed).
r2e-mcp         → MCP (Model Context Protocol) server: `#[mcp_routes]` + `#[tool]` on a dedicated type (gRPC transport-adapter model) expose tools over rmcp's streamable-HTTP transport, dispatched by R2E (rmcp = wire only, `default-features = false`). Schemas from `schemars` (`Params<T>` → inputSchema, `Json<T>` → outputSchema + structuredContent), guards/interceptors SHARED with HTTP (`Guard<I>`/`DecoratorSpec`, prebuilt at `register_mcp_service` with compile-time `EndpointDeps` check), `McpServer` plugin (config `mcp.*`, path validation, shared session map across SO_REUSEPORT workers, shutdown-token relay terminating SSE streams). Duplicate tool name across services = boot panic. Auth (`mcp.auth.*`): IdP-agnostic OAuth 2.1 resource server — issuer discovery (RFC 8414 incl. path-insertion), local JWT validation via `McpTokenValidator` (JWKS, aud = canonical resource URI from `server.public-url`), RFC 9728 protected-resource metadata + `WWW-Authenticate` challenges, static DCR shim (`public-client-id`), per-tool `#[tool(scopes/any_scopes)]` + shared `#[roles]` guards, `tools/list` filtering; test fast path `pin_mcp_validator` (feature `testing` / facade `mcp-testing`).
r2e-cache       → TtlCache, pluggable CacheStore trait. The store is a bean: `.provide(InMemoryStore::shared())` (no global).
r2e-rate-limit  → Token-bucket RateLimiter, pluggable RateLimitBackend, RateLimitRegistry.
r2e-openapi     → OpenAPI 3.1.0 spec generation, Swagger UI at /docs.
r2e-prometheus   → Prometheus metrics plugin: HTTP request tracking, /metrics endpoint.
r2e-observability → OpenTelemetry plugin: distributed tracing and context propagation via OTLP.
r2e-oidc        → Embedded OIDC server plugin: issue JWTs without an external IdP.
r2e-openfga     → OpenFGA fine-grained authorization: Zanzibar-style relationship-based access control. Schema-first typed API via `model!` (guards: `FgaCheck::has(authz::document::viewer)`). `OpenFga` plugin owns the store lifecycle at boot: ensure/create store, apply model when changed (dev) or verify + fail-fast (prod, `openfga.apply_model: false`), pinned `model_id`.
  model/            → r2e-openfga-model: pure `.fga` DSL parser (schema 1.1) → typed model → JSON. Syntax (`parse`) and semantic (`validate`) layers; validated against the vendored openfga/language corpus. No proc-macro deps.
  macros/           → r2e-openfga-macros: `model!(pub mod authz = "fga/model.fga")` — compile-time parse+validate, generates typed markers (`FgaType`/`FgaRel` consts/`DirectlyAssignable` impls) + `authz::MODEL` JSON.
r2e-utils       → Built-in interceptors: Logged, Timed, Cache, CacheInvalidate.
r2e-test        → TestApp (HTTP client + App boot: TestApp::boot::<A>() / #[r2e::test(app = MyApp)], bean::<T>() access, .as_user() via auto-wired TestJwt), TestJwt (local HS256 tokens + validators), TestSession (cookie persistence), assertion helpers (JSON contains/shape/path), TestServer (live TCP), WsTestClient (WebSocket, feature "ws"), FiniteStream/ParsedSseEvent (SSE), SetCookie (cookie attributes), multipart file upload builders.
r2e-devservices → Dev services for tests (testcontainers): DevPostgres/DevRedis, workspace-session shared containers reaped by Ryuk after the final test process exits, URL injected via override_config_value. `DevPostgres`/`DevRedis` take a full spec on both the isolated and shared paths — image (`PostgresImage::new("pgvector/pgvector", "pg18")`, `RedisImage::new("valkey/valkey", "8-alpine")`) plus credentials for Postgres (`PostgresSpec::with_user/with_password/with_database`) — one shared container per distinct spec. `DevService`/`DevServiceSpec` (ungated) is the generic form they are built on: any testcontainers `Image` gets the same labelling/reaping/sharing (`testcontainers` + `testcontainers_modules` re-exported). Features `postgres`, `redis`, `openfga`.
r2e-devtools    → Subsecond hot-reload support (wraps dioxus-devtools). Feature-gated behind `dev-reload`.
r2e-static      → Embedded static file serving with SPA support. Plugin-based, wraps rust_embed.
r2e-tenant      → Multi-tenant bean routing. `TenantResolver` (request → `TenantId`) + `TenantSource<T>` (tenant → `T`) + `Tenanted<T>` (per-tenant map: single-flight create, negative cache, idle/LRU eviction with dispose, drain on shutdown). Request-scoped `Tenant<T>` / `TenantId` extractors (`FromRequestPartsVia` + `ViaBean` — no axum `FromRequestParts` impl, so a missing plugin is a compile error). Two plugins: `Tenancy::resolver::<R>()` (provides `TenantRouter`) and `PerTenant::<T>::from::<Src>()` (provides `Tenanted<T>`). Config under `tenancy.*`.
r2e-cli         → CLI: r2e new, r2e add, r2e dev, r2e generate, r2e doctor, r2e routes, r2e docs (bundled per-module TL;DR docs).
r2e-compile-tests → Compile-time tests (trybuild) verifying macro error messages.
example-app     → Demo app (lib + bin) exercising all features. `lib.rs` declares the app via `impl App for ...` (`setup`/`build`); `main.rs` runs `r2e::launch::<App>()` and the integration tests boot the same type via `#[r2e::test(app = ...)]`.
```

Dependency flow: `r2e-rt` ← `r2e-http` ← `r2e-macros` ← `r2e-core` ← `r2e-security` / `r2e-events` / `r2e-scheduler` / `r2e-devtools` / `r2e-static` / `r2e-tenant` / `r2e-data-sqlx` / `r2e-data-diesel` / other integrations ← `r2e` ← applications. The data backends' `tenant` feature adds `r2e-tenant`, so `r2e-tenant` precedes them.

**Only `r2e-http` depends on `axum` directly.** All other crates access HTTP types through `r2e_core::http` (which re-exports from `r2e-http`). R2E code implements **R2E's** contracts, not the backend's: `IntoHttpResponse` + `r2e::http::impl_into_response!(Ty)` for responses, `FromRequestPartsVia`/`Via<T, M>` for extraction. The few surviving impls of axum's own traits are named bridge points, tabulated in `plans/runtime-http-dependency-containment.md` §5.3b and annotated in the source. The public promise to users is "R2E types" (§5.3d decision A, 2026-08-24); raw axum is reachable only through `r2e::http::axum_compat`.

**`r2e_core::json` is the same rule for the JSON codec**: typed (de)serialization (`to_vec`, `to_string`, `from_slice`, `from_str`) goes through the façade, never `serde_json::…` directly. `serde_json::Value` / `json!` / `to_value` / `from_value` are the dynamic-tree type and deliberately stay `serde_json` — only codec calls are counted by the boundary check (`serde_json` group, `plans/json-codec-containment.md` §1.3). JWT claims are typed (`StandardClaims`, in `r2e-core`), not `Value`.

**`r2e-rt` is the same rule for the runtime**: go through `r2e_core::rt` (or `r2e_rt` directly, in crates below `r2e-core`) rather than naming `tokio` / `tokio-util` / `tokio-stream`. Both boundaries are enforced in CI by `scripts/check-dep-boundary.sh` (manifests) and `scripts/check-source-boundary.sh` (source occurrences, against a baseline that only ever shrinks). Migration status and the by-design exceptions (`r2e-rt` itself plus the `r2e-test` / `r2e-devservices` harnesses, which own a runtime on purpose) live in `plans/runtime-http-dependency-containment.md`. Both baselines are at their end state: the tokio source baseline is **empty** and the tokio dep allowlist is exactly those three crates; the axum source baseline is confined to `r2e-http/src/`.

### Vendored Dependencies

`vendor/openfga-rs/` — patched copy using tonic ~0.12 with `features = ["tls", "channel", "codegen", "prost"]` to avoid dual axum-core conflict. See `vendor/README.md`.

### Core Concepts

**The application state is inferred** — there is no hand-written state struct. `AppBuilder::new().provide(bean).register::<T>().build_state().await` materializes the compile-time provision list `P` into a type-level HList of resolved beans (the axum state). Beans are read by type: `state.get::<T>()` (via `BeanAccess`, NOT in the prelude — import explicitly) monomorphizes to a fixed-offset field access; `BeanLookup` (`state.bean::<T>() -> Option<T>`) is the witness-free dynamic form used by `ManagedResource`. Guards/interceptors do NOT read the state: they are built once at registration via `DecoratorSpec` (`#[guard]`/`#[intercept]` expressions name a spec type; bean deps are fields, folded into `Controller::Deps` and compile-checked). The resolved graph is also retained as `Arc<BeanContext>` on the typed builder (`bean_context()`). Apps with >~127 registrations need `#![recursion_limit = "512"]` at the crate root.

**Four injection scopes, all resolved at compile time — two app-scoped, two request-scoped:**
- `#[inject]` — App-scoped. Resolved from the bean graph BY TYPE (`ctx.get::<FieldType>()`) at registration. Type must be `Clone + Send + Sync + 'static` and provided/registered on the builder — a missing bean is a compile error at `register_controller`. Lives on the controller core (built once).
- `#[config("key")]` — App-scoped. Resolved from `R2eConfig`. Type must implement `FromConfigValue`. Lives on the controller core.
- `#[inject(identity)]` — Request-scoped. Extracted via `FromRequestParts` (e.g., `AuthenticatedUser`). Type must implement `Identity`. Drives guards/roles. Lives on the per-request façade.
- `#[inject(request)]` — Request-scoped. Any type implementing `FromRequestParts` (e.g. a tenant id, correlation/trace context, a request-scoped handle). Use it for everything request-scoped that is *not* the auth identity. Lives on the per-request façade. (Not modeled in OpenAPI yet.)

`Option<T>` is supported for both `#[inject(identity)]` and `#[inject(request)]`.

**Handler parameter-level identity injection:**
- `#[inject(identity)]` on handler parameters enables mixed controllers (public + protected endpoints), with each endpoint opting into authentication individually.
- **Optional identity:** `#[inject(identity)] user: Option<AuthenticatedUser>` for endpoints working with or without auth.

**`#[anonymous]` — fail-closed auth with per-route opt-out:** a struct-level identity authenticates **every** route by default; mark the public exceptions with `#[anonymous]` (@PermitAll-style). Anonymous routes are emitted on the controller **core** (like consumers/scheduled): identity extraction is skipped entirely (no JWT cost) and reading the identity or any request-scoped field in the body is a compile error. Guards still run there — with `identity: None` unless the route declares its own optional identity param; OpenAPI drops the security requirement unless explicit `#[guard]`s remain. Rejected combinations (compile errors): `#[anonymous]` + `#[roles]`/`#[all_roles]`, + a **required** `#[inject(identity)]` param (an `Option<T>` identity param is allowed — adaptive public route), or on a controller without a **required** struct identity (no identity or `Option<T>` identity = nothing fail-closed to opt out of — const-assert on `STRUCT_IDENTITY_IS_REQUIRED`). Prefer struct identity + `#[anonymous]` for mostly-protected controllers (forgetting the marker fails closed with a 401); use param-level identity for mostly-public ones.

**Controller declaration uses two macros:**
1. `#[controller(path = "...")]` — a transforming attribute on the struct (no `state` key — controllers are state-generic). It strips request-scoped fields from the physical core struct and generates the metadata module, the request-data extractor, the per-request façade, and the `ContextConstruct` impl (always — the core never holds request-scoped fields).
2. `#[routes]` on the impl block — generates Axum handler functions and the state-generic `Controller<S, W>` trait impl (`S: Clone + Send + Sync + 'static + BeanLookup`; `W` carries inferred extraction markers). Route methods run on the generated façade.

```rust
#[controller(path = "/users")]
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
```

**Generated items (hidden):**
- A physical **core** struct (the source struct with request-scoped fields stripped) — holds `#[inject]` + `#[config]` fields plus a hidden `__r2e_decos: DecoSlot` (prebuilt `#[scheduled]`/`#[consumer]`-method interceptor sets, filled at registration via `Controller::fill_decos`), built once into an `Arc` by `register_controller()`. Cores are not literal-constructible — build via `ContextConstruct::from_context`. The controller core reuses the same bean-level transverse machinery (`r2e-macros/src/codegen/transverse.rs`) for `#[scheduled]`/`#[consumer]`/`#[intercept]`/`#[post_construct]` ("the controller core IS a bean").
- `mod __r2e_meta_<Name>` — `type IdentityType`, `const PATH_PREFIX`, `fn guard_identity()`, `fn bind_request()`, `fn validate_config()`.
- `struct __R2eRequestData_<Name><__M>` — state-generic `FromRequestParts` extractor for the request-scoped values (identity + `#[inject(request)]`), extracted through `FromRequestPartsVia<S, M>` (R2E-owned trait with a marker slot where bean-backed extractors park their `HasBean` index witnesses — E0207). Marker-only + infallible when there are none.
- `struct __R2eRequest_<Name>` — the per-request façade: `{ __core: Arc<Core>, <request-scoped fields> }`, with `Deref<Target = Core>`. Route methods run on this; `self.<injected/config>` resolves through `Deref`, `self.<identity/request>` is a direct façade field.
- `impl ContextConstruct for Name` — always generated; `from_context(ctx)` pulls each `#[inject]` field with `ctx.get::<Ty>()` and declares `type Deps` (checked via `AllSatisfied` at registration).
- `impl<S, ...markers> Controller<S, W> for Name` — receives the core built by `register_controller()` (an extension-trait method: `RegisterController`/`RegisterControllers`, in the prelude) and wires routes, consumers, and scheduled tasks to that same instance. Per request: one `Arc` clone of the core + one `FromRequestParts` extraction binding the stack façade. No DI re-resolution per request, no `Extension<Arc<Controller>>`, no task-local identity.

### Macro Crate Internals (r2e-macros)

`src/` is grouped by role: `attrs/` (transforming attribute macros: `bean_attr`, `controller_attr`, `main_attr`, `module_attr`, `producer_attr`, `routes_attr`, `test_suite_attr`), `derives/` (derive macros: `api_error_derive`, `config_derive`, `params_derive`, …), `parsing/` (`controller_parsing`, `routes_parsing`, `grpc_routes_parsing`), `codegen/` (emission: `controller_codegen`, `controller_impl`, `handlers`, `decorators`, `scheduled`, `transverse`, `wrapping`), `model/` (shared parsed-definition types), `util/` (`crate_path`, `type_utils`, `hash_tokens`, `runtime_args`), plus `extract/` and `grpc_codegen/`.

**Controller path:** `lib.rs` → `attrs/controller_attr.rs` → `parsing/controller_parsing.rs` (`ControllerStructDef`) → `codegen/controller_codegen.rs`

**Routes path:** `lib.rs` → `attrs/routes_attr.rs` → `parsing/routes_parsing.rs` (`RoutesImplDef`) → `codegen/` (handlers, controller_impl, …)

**Shared modules:**
- `model/types.rs` — `InjectedField`, `IdentityField`, `RequestField`, `ConfigField`, `RouteMethod`, `ConsumerMethod`, `ScheduledMethod`, etc.
- `model/route.rs` — `HttpMethod` enum and `RoutePath` parser
- `extract/` — attribute extraction (`route`, `consumer`, `scheduled`, `async_exec`, `managed`, `plugins`, `duration`)

**Inter-macro liaison:** `#[controller]` generates `__r2e_meta_<Name>` (with `bind_request`), `__R2eRequestData_<Name>`, and the `__R2eRequest_<Name>` façade. `#[routes]` references these by naming convention and emits route methods on the façade.

**No-op attribute macros:** `#[get]`, `#[any]`, `#[fallback]`, `#[roles]`, `#[anonymous]`, `#[intercept]`, `#[guard]`, `#[consumer]`, `#[scheduled]`, `#[middleware]`, `#[post_construct]`, `#[on_start]`, `#[pre_destroy]`, etc. are no-op `#[proc_macro_attribute]` parsed by `#[routes]` or `#[bean]`. `#[inject]` (incl. `#[inject(identity)]` / `#[inject(request)]`), `#[config]`, and `#[config_section]` are field helper attributes consumed by `#[controller]`.

**`#[post_construct]`** — lifecycle hook on `#[bean]` methods **and on `#[routes]` controller impls**. `&self` only, may be async, returns `()` or `Result<(), Box<dyn Error + Send + Sync>>`. Generates a `PostConstruct` trait impl. Timing differs by host: bean hooks run inside `build_state()` (after the graph resolves, before subscribers); controller-core hooks run at startup during `register_controller`/`build_with_consumers`, **before** consumer registrations (later than bean hooks, since cores are built after the graph). An `Err` aborts startup. On controllers, `#[post_construct]` combined with a route/`#[scheduled]`/`#[consumer]` marker, or with params, or with `#[intercept]`, is a compile error.

**`#[on_start]`** — startup observer on `#[bean]` methods **and** `#[routes]`
controller impls. Same signature/rejection rules as `#[post_construct]`, plus an
optional `#[on_start(order = N)]` (`i32`, default 0). Runs at boot **after** the
whole graph and every controller core are built (so a hook may read anything the
app declares) and after consumer registrations, **before** the plugins' serve
hooks, the builder's `.on_start` closures and the TCP bind. All hooks (beans +
controllers) are sorted ascending by `order`, ties in registration order; an
`Err` **aborts boot** like a builder `.on_start` error. A pinned `override_bean`
skips the hook. Unlike `#[pre_destroy]`, it **does** run under
`build_with_consumers` / `TestApp::boot` (a test boot is a real startup; it just
never shuts down). Generates `impl OnStart` + `register_on_start` on beans, and
the `Controller::on_start(core)` override on controllers.

**`#[pre_destroy]`** — disposal hook (the `@PreDestroy` counterpart of `#[post_construct]`), on `#[bean]` methods **and** `#[routes]` controller impls. Same signature/rejection rules as `#[post_construct]`. Runs at **graceful shutdown** in the async shutdown phase — controller hooks first, then bean hooks, each in reverse registration order. An `Err` is logged and swallowed (never aborts shutdown); a pinned `override_bean` skips the hook. `#[bean]` generates `impl PreDestroy` + `register_pre_destroy`; a controller core (not `Clone`) uses the `Controller::pre_destroy(core)` override. Does NOT fire on `build_with_consumers`/`TestApp` (no shutdown) — test via serve + `StopHandle::stop()`.

## Detailed Reference — Read Before You Code

**DO NOT guess APIs or patterns. Match your task to the keyword table below and READ only the matching file(s).** Each file is the authoritative source for its subsystem. Reading all files wastes context — be selective.

### Keyword → Doc routing table

| If your task involves… | Read this file |
|---|---|
| `R2eConfig`, `ConfigProperties`, `ConfigValue`, `FromConfigValue`, `#[config(...)]`, `load_config`, `with_config`, secrets (`${...}`), YAML config, typed sections, `#[config(section)]`, env overlay, `serve_auto` | `docs/claude/configuration.md` |
| `Guard`, `PreAuthGuard`, `GuardContext`, `#[guard]`, `#[roles]`, `Identity`, `RolesGuard`, `RateLimitGuard`, `PreRateLimit`, `Interceptor`, `#[intercept]`, `DecoratorSpec`, `SelfBuilt`, `#[derive(DecoratorBean)]`, `build_decorator`, `Logged`, `Timed`, `Cache` store bean, middleware ordering | `docs/claude/guards-interceptors.md` |
| OpenFGA schema-first: `model!`, `.fga` DSL parser, `FgaCheck::has`, `FgaType`/`FgaRel`/`FgaObject`, `DirectlyAssignable`, `authz::MODEL` | `docs/features/23-openfga.md` (user guide) + `docs/claude/roadmap.md` § W12 |
| multi-tenancy, `TenantId`, `Tenant<T>`, `Tenanted<T>`, `TenantResolver`/`SyncTenantResolver`, `TenantSource`, `TenantContext` (cascade), `Tenancy`/`PerTenant` plugins, `TenantRouter`, `tenancy.*` config, `TenantError` statuses, `TenantPools`/`TenantTx`/`PoolSource` (per-tenant SQLx/Diesel), `.as_tenant()` in tests | `docs/features/24-tenancy.md` (user guide) + `docs/claude/subsystems.md` § r2e-tenant (internals) |
| controller lifetime, controller reconstruction, struct-level identity, parameter identity, request façade, `Controller::routes(&state, core, ctx)`, handler generation, controller codegen performance | `docs/claude/controller-identity-codegen-refactor.md` |
| `HttpError`, `ApiError`, `#[derive(ApiError)]`, `map_error!`, validation, `garde`, `ManagedResource`, `#[managed]`, error responses | `docs/claude/error-handling.md` |
| `Bean`, `AsyncBean`, `Producer`, `#[bean]`, `#[producer]`, `#[inject]`, `#[post_construct]`, `BeanRegistry`, `BeanContext`, `build_state`, dependency injection, bean graph | `docs/claude/beans-di.md` |
| `Plugin`, `PluginInstall`, `.plugin()`, `Provided`/`PluginProvisions`, `Deps`, `Controllers`, `configure`, plugin `Config`/`CONFIG_PREFIX`/`PluginConfig`, effect stages (Graph/Routes/Finalize), `after_routes`/`RoutesContext`, `HealthRegistry`, `DeferredAction`/`DeferredContext`, `store_data`/plugin data, writing a new plugin | `docs/claude/plugins.md` |
| `PoolExecutor`, `JobHandle`, `Executor` plugin, `ExecutorConfig`, `#[async_exec]`, `#[derive(BackgroundService)]`, `ServiceComponent`, `spawn_service`, managed task pool, background workers | `docs/claude/executor.md` |
| `Cache`, `TtlCache`, `RateLimiter`, `RateLimitRegistry`, `AuthenticatedUser`, `JwksValidator`, `EventBus`, `#[consumer]`, `#[scheduled]`, `Scheduler`, managed SQLx/Diesel transactions, `Pageable`, `Page`, `OpenAPI`, `ContextConstruct`, `AppBuilder`, `TestApp`, `TestJwt`, `TracingConfig`, `LogFormat`, `SpanEvents`, `ConfiguredTracing`, `init_tracing_with_config`, tracing subscriber formatting | `docs/claude/subsystems.md` |
| `prelude`, `use r2e::prelude::*`, feature flags, `Params`, re-exports, what's available by default | `docs/claude/prelude-features.md` |
| `r2e new`, `r2e dev`, `r2e generate`, `r2e add`, `r2e doctor`, `r2e routes`, CLI templates, scaffolding | `docs/claude/cli.md` |
| `quic`, `quinn`, `h3`, HTTP/3, `serve_h3`, `QuicEndpoint`, `QuicConnection`, `Alt-Svc`, `build_server_config`, raw QUIC streams, `server.quic.*` | `docs/features/18-quic.md` |
| `server.workers`, `per-core`, SO_REUSEPORT, sharded serving, thread-per-core, `parse_workers`, `MAX_WORKERS`, `ServeStrategy`, `rt::spawn`, `spawn_ctl`, `set_control_plane`, control plane / data plane, worker runtimes | `docs/features/19-sharded-serving.md` |
| `#[any]`, `#[fallback]`, catch-all, wildcard `{*path}` routes, proxy/gateway routing, raw `Request` param, streaming responses (`Body::from_stream`), escape-hatch ladder (`merge_router`, `with_layer_fn`) | `docs/features/20-proxy-catch-all.md` |
| `StopHandle`, programmatic stop, graceful shutdown/drain, `on_drain`, `on_stop`, `ServeContext`, `on_serve`, `track`, shutdown token, `shutdown_grace_period`, gRPC drain, readiness flip / LB deregistration | `docs/features/22-serve-lifecycle.md` |
| dev-reload config semantics, hot-patch cycles, cached state, partial rebuild, `provided_reuse_clones`, pinned provided values, graph fingerprint scope, `LiveConfigRegistry` identity across cycles, stale typed config under `r2e dev`, `crate::dev` statics | `docs/claude/dev-reload-config-semantics.md` |
| DI/builder refactor status & phases, `.register()`, `build_state()`, HList state, `HasBean`/`BeanLookup`/`BeanAccess`, `FromRequestPartsVia`, `.when()`, `register_controllers`, unified registration, `recursion_limit` | `docs/claude/di-builder-refactor.md` |
| feature modules, `#[module]`, `register_module`, closed subgraph, module imports/exports/encapsulation, modules bringing plugins (`plugins(Type = expr)`) / `requires_plugins`, controllers as beans, `from_context`, `ContextConstruct`, context-as-state | `docs/claude/di-builder-refactor.md` |
| guards/interceptors as beans, `DecoratorSpec`, `DecoratorBean`, guard compile-time deps, once-at-registration guard construction, `Guard<I>`/`Interceptor<R>` redesign | `docs/claude/guards-interceptors.md` |
| roadmap, backlog, next steps, what to work on, framework gaps, real-app audit (threaty/patina), rejected-design decisions (qualifiers, startup_check) | `docs/claude/roadmap.md` |
| fixing audit findings on hard concurrent code, adversarial verification loop, codex verify passes, persistent fixer agent, mutation checks, merge-verdict exit | `docs/claude/methodology-adversarial-fix-loop.md` |
| EventBus perf/reliability work, distributed backend audit (iggy/kafka/pulsar/rabbitmq), delivery semantics (at-least-once, ack-after-handler), producer batching, `BackendState` dedup/event_id, consume pipelining | `docs/claude/eventbus-perf.md` |
| MCP server, `#[mcp_routes]`, `#[tool]`, `McpServer`, `register_mcp_service`, `Params<T>`, `ToolCall`, `McpError`, `IntoToolResult`, `mcp.*` config, MCP guards/interceptors, streamable HTTP, MCP sessions, `mcp.allowed-hosts`, MCP auth, `mcp.auth.*`, OAuth resource server, PRM (RFC 9728), DCR shim, `McpTokenValidator`, `ScopePolicy`, `#[tool(scopes)]`, `pin_mcp_validator`, `server.public-url` | `docs/features/25-mcp.md` (user guide) + `docs/claude/transport-adapters.md` (adapter internals) |
| new transport / wire adapter, `EndpointDeps`, `endpoint_deps_fold`, `register_grpc_service` compile check, `AppBuilderGrpcExt`, ports-and-adapters shape, per-transport guards decision | `docs/claude/transport-adapters.md` |
| testing DX, `App` trait, `App::setup`/`App::build`, `r2e::launch`, `override_config`, `BootableApp`, `TestApp::boot`, `#[r2e::test(app = ...)]`, `override_bean` (pinned overrides), `override_config_value`, `with_profile`, `application-test.yaml`, `.as_user()`, mocks in tests, dev services / testcontainers | `docs/claude/subsystems.md` (TestApp section); open follow-ups in `docs/claude/roadmap.md` |

**Rules:**
1. Match keywords from your task to the left column. Read **only** the matched file(s).
2. If your task spans two subsystems (e.g., config + beans), read both — but no more.
3. If nothing matches, you probably don't need a reference doc. Proceed with the code.

## Keeping `llm.txt` Fresh

`llm.txt` at the repo root is the canonical AI/agent-facing reference that downstream projects rely on. **Any change to a public API surface (traits, macros, builder methods, renames, removals) MUST update the matching `llm.txt` section in the same PR.** Code agents in consumer apps follow `llm.txt` literally — a stale example (e.g. a removed method) makes them generate non-compiling code.

## Language & Documentation

All documentation, code, comments, and API surfaces are in English.
