# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

AGENTS.md-aware agents are bound through `AGENTS.md`, which delegates to this
file. Keep cross-agent guidance changes visible from both `CLAUDE.md` and
`AGENTS.md`.

This file is a **hub**: golden rules + a routing table. Subsystem detail lives in
`docs/claude/*.md` and `docs/features/*.md` — match your task to the routing table below
and read only the matched file(s).

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

**Tests live in `<crate>/tests/` directories, not inline** — no `#[cfg(test)] mod tests`
blocks in source files. One test module per source module; modules are grouped into **one
Cargo target per subsystem** (`tests/<name>/main.rs` + plain `mod`s) — do NOT add new
top-level `tests/*.rs` files to a crate that already uses this layout; add a `mod` to the
matching target. Use external imports (`use <crate_name>::...`), keep helpers in the test
file, and expose internals via `pub` + `#[doc(hidden)]` (never `#[cfg(test)] pub(crate)`).
Env-mutating tests take `crate::support::env_lock()`. `r2e-core/tests/` is the reference
layout — full target map and conventions: `docs/claude/testing-conventions.md`.

```bash
cargo test -p r2e-core --test config              # one subsystem target
cargo test -p r2e-core --test config sections::   # one module within it
```

## Architecture

R2E is a **Quarkus-like ergonomic layer over Axum** for Rust: declarative controllers with
compile-time dependency injection, JWT/OIDC security, zero runtime reflection.

### Workspace Crates (one line each — full detail: `docs/claude/architecture.md`)

```
r2e               → Facade crate; re-exports subcrates behind feature flags. Users depend on this.
r2e-rt            → Async-runtime facade; the ONLY crate naming tokio; bottom of the graph; re-exported as r2e_core::rt.
r2e-macros        → Proc macros: #[controller] + #[routes] generate Axum handlers.
r2e-http          → HTTP abstraction; sole owner of axum; IntoHttpResponse, axum_compat escape hatch, json codec façade, QUIC/HTTP3.
r2e-core          → Runtime foundation: AppBuilder → HList state, Controller trait, guards/interceptors, R2eConfig, HttpError, lifecycle, ManagedResource, Page/Pageable. Re-exports r2e-http as `http`.
r2e-security      → JWT/JWKS validation, AuthenticatedUser, RoleExtractor; feature grpc shares the validator with gRPC.
r2e-events        → In-process EventBus (typed pub/sub + request-reply); distributed backends in backends/ (iggy, kafka, pulsar, rabbitmq).
r2e-scheduler     → Interval/cron scheduling on the Executor pool (single driver task); overlap/skip_if policies; SchedulerHandle; scheduler.* config.
r2e-executor      → Managed task pool (PoolExecutor), #[async_exec], #[derive(BackgroundService)]; bounded concurrency, graceful drain.
r2e-data/backends/{sqlx,diesel} → Managed transactions + DataSource plugins (datasource.* config, migrate-at-start, DataSourceHealth); feature tenant: per-tenant pools + TenantTx.
r2e-grpc (+build/) → Tonic gRPC, separate port or HTTP-multiplexed; grpc-web; r2e-grpc-build proto build helper.
r2e-mcp           → MCP server: #[mcp_routes] + #[tool]/#[resource]/#[prompt]; guards shared with HTTP; McpServer plugin; OAuth 2.1 resource-server auth (mcp.auth.*).
r2e-cache         → TtlCache + pluggable CacheStore bean (no global).
r2e-rate-limit    → Token-bucket RateLimiter, pluggable backend, RateLimitRegistry.
r2e-openapi       → OpenAPI 3.1.0 generation, Swagger UI at /docs.
r2e-prometheus    → HTTP metrics; three modes (full / layer_only / MetricsFacade) over the HttpMetricsRecorder seam.
r2e-observability → OpenTelemetry tracing + context propagation via OTLP.
r2e-oidc          → Embedded OAuth/JWT issuer (RS256, PKCE code flow, client_credentials; no ID tokens/federation).
r2e-openfga (+model/, macros/) → OpenFGA authorization: .fga parser, model! macro, typed FgaCheck guards, store-lifecycle plugin.
r2e-utils         → Built-in interceptors: Logged, Timed, Cache, CacheInvalidate.
r2e-test          → TestApp (real boot + shutdown), TestJwt, TestSession, TestServer, WsTestClient, SSE helpers, assertions.
r2e-devservices   → Testcontainers dev services: DevPostgres/DevRedis/DevKeycloak + generic DevService; workspace-shared containers.
r2e-devtools      → Subsecond hot-reload (feature dev-reload).
r2e-static        → Embedded static files / SPA (rust_embed, plugin-based).
r2e-tenant        → Multi-tenant bean routing: TenantResolver/TenantSource/Tenanted<T>, Tenancy + PerTenant plugins, tenancy.* config.
r2e-cli           → r2e new/add/dev/generate/doctor/routes/docs.
r2e-compile-tests → trybuild tests for macro error messages.
example-app       → Demo app (App trait in lib.rs; main.rs + integration tests boot the same type).
```

Dependency flow: `r2e-rt` ← `r2e-http` ← `r2e-macros` ← `r2e-core` ← integrations (`r2e-security`, `r2e-events`, `r2e-tenant`, data backends, …) ← `r2e` ← applications.

### Containment Boundaries (CI-enforced — details: `docs/claude/architecture.md`)

- **axum**: only `r2e-http` depends on it. Everything else goes through `r2e_core::http` and implements **R2E's** contracts (`IntoHttpResponse` + `impl_into_response!`, `FromRequestPartsVia`/`Via<T, M>`); raw axum only via `r2e::http::axum_compat`.
- **JSON codec**: typed (de)serialization goes through `r2e_core::json` (`to_vec`/`from_slice`/…), never `serde_json::…` directly. `serde_json::Value` / `json!` (dynamic tree) deliberately stays `serde_json`. JWT claims are typed (`StandardClaims`), not `Value`.
- **tokio**: go through `r2e_core::rt` (or `r2e_rt` below core), never `tokio`/`tokio-util`/`tokio-stream`. By-design exceptions: `r2e-rt`, `r2e-test`, `r2e-devservices`.

Enforced by `scripts/check-dep-boundary.sh` + `scripts/check-source-boundary.sh` (baselines only ever shrink).

### Core Concepts (summary — full detail: `docs/claude/core-concepts.md`)

- **State is inferred** — no hand-written state struct. `AppBuilder::new().provide(..).register::<T>().build_state().await` → type-level HList of beans (the axum state). Read by type via `state.get::<T>()` (`BeanAccess`, NOT in the prelude). >~127 registrations need `#![recursion_limit = "512"]`.
- **Four compile-time injection scopes**: `#[inject]` (app-scoped bean, by type; missing bean = compile error), `#[config("key")]` (app-scoped config), `#[inject(identity)]` (request-scoped auth identity — drives guards/roles), `#[inject(request)]` (any other request-scoped `FromRequestParts`). `Option<T>` supported on both request scopes; identity can also be a handler **parameter** for mixed public/protected controllers.
- **Fail-closed auth**: a required struct-level identity authenticates every route; opt public routes out with `#[anonymous]` (identity extraction skipped entirely; combining it with `#[roles]` or a required identity param is a compile error).
- **Controllers = two macros**: `#[controller(path = "...")]` on the struct (strips request-scoped fields, generates metadata + extractor + per-request façade + `ContextConstruct`) and `#[routes]` on the impl (generates handlers + state-generic `Controller<S, W>` impl). Register with `register_controller()`. Per request: one `Arc` clone of the core + one extraction — no per-request DI.
- **Guards/interceptors don't read the state**: built once at registration via `DecoratorSpec`; bean deps are compile-checked through `Controller::Deps`.
- **Lifecycle hooks on beans AND controller impls**: `#[post_construct]` (build time; `Err` aborts startup), `#[on_start(order = N)]` (boot, after graph + cores; `Err` aborts), `#[pre_destroy]` (graceful shutdown, reverse order; `Err` logged and swallowed).

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

## Detailed Reference — Read Before You Code

**DO NOT guess APIs or patterns. Match your task to the keyword table below and READ only the matching file(s).** Each file is the authoritative source for its subsystem. Reading all files wastes context — be selective.

### Keyword → Doc routing table

| If your task involves… | Read this file |
|---|---|
| workspace layout, crate responsibilities in detail, dependency flow, axum/json/tokio boundary baselines and bridge points, checked-in generated code (OpenFGA proto client) | `docs/claude/architecture.md` |
| test layout, adding a test target/module, `tests/<name>/main.rs` grouping, fixtures, `env_lock()` | `docs/claude/testing-conventions.md` |
| injection scopes detail, `#[anonymous]` rules, generated items (`__r2e_meta`, façade, request-data extractor), lifecycle hook semantics (`#[post_construct]`/`#[on_start]`/`#[pre_destroy]`), r2e-macros internal layout | `docs/claude/core-concepts.md` |
| `R2eConfig`, `ConfigProperties`, `ConfigValue`, `FromConfigValue`, `#[config(...)]`, `load_config`, `with_config`, secrets (`${...}`), YAML config, typed sections, `#[config(section)]`, env overlay, `serve_auto` | `docs/claude/configuration.md` |
| `Guard`, `PreAuthGuard`, `GuardContext`, `#[guard]`, `#[roles]`, `Identity`, `RolesGuard`, `RateLimitGuard`, `PreRateLimit`, `Interceptor`, `#[intercept]`, `DecoratorSpec`, `SelfBuilt`, `#[derive(DecoratorBean)]`, `build_decorator`, `Logged`, `Timed`, `Cache` store bean, middleware ordering | `docs/claude/guards-interceptors.md` |
| OpenFGA schema-first: `model!`, `.fga` DSL parser, `FgaCheck::has`, `FgaType`/`FgaRel`/`FgaObject`, `DirectlyAssignable`, `authz::MODEL` | `docs/features/23-openfga.md` (user guide) + `docs/claude/roadmap.md` § W12 |
| multi-tenancy, `TenantId`, `Tenant<T>`, `Tenanted<T>`, `TenantResolver`/`SyncTenantResolver`, `TenantSource`, `TenantContext` (cascade), `Tenancy`/`PerTenant` plugins, `TenantRouter`, `tenancy.*` config, `TenantError` statuses, `TenantPools`/`TenantTx`/`PoolSource` (per-tenant SQLx/Diesel), `.as_tenant()` in tests | `docs/features/24-tenancy.md` (user guide) + `docs/claude/subsystems.md` § r2e-tenant (internals) |
| controller lifetime, controller reconstruction, struct-level identity, parameter identity, request façade, `Controller::routes(&state, core, ctx)`, handler generation, controller codegen performance | `docs/claude/controller-identity-codegen-refactor.md` |
| `HttpError`, `ApiError`, `#[derive(ApiError)]`, `map_error!`, validation, `garde`, `ManagedResource`, `#[managed]`, error responses | `docs/claude/error-handling.md` |
| `Bean`, `AsyncBean`, `Producer`, `#[bean]`, `#[producer]`, `#[inject]`, `#[post_construct]`, `BeanRegistry`, `BeanContext`, `build_state`, dependency injection, bean graph | `docs/claude/beans-di.md` |
| `Plugin`, `PluginInstall`, `.plugin()`, `Provided`/`PluginProvisions`, `Deps`, `Controllers`, `configure`, plugin `Config`/`CONFIG_PREFIX`/`PluginConfig`, effect stages (Graph/Routes/Finalize), `after_routes`/`RoutesContext`, `HealthRegistry`, `DeferredAction`/`DeferredContext`, `store_data`/plugin data, writing a new plugin | `docs/claude/plugins.md` |
| `PoolExecutor`, `JobHandle`, `Executor` plugin, `ExecutorConfig`, `#[async_exec]`, `#[derive(BackgroundService)]`, `ServiceComponent`, `spawn_service`, managed task pool, background workers | `docs/claude/executor.md` |
| `Cache`, `TtlCache`, `RateLimiter`, `RateLimitRegistry`, `AuthenticatedUser`, `JwksValidator`, `EventBus`, `#[consumer]`, `#[scheduled]`, `Scheduler`, managed SQLx/Diesel transactions, `Pageable`, `Page`, `OpenAPI`, `ContextConstruct`, `AppBuilder`, `TestApp`, `TestJwt`, `TracingConfig`, `LogFormat`, `SpanEvents`, `ConfiguredTracing`, `init_tracing_with_config`, tracing subscriber formatting, `HttpTrace`, `HttpTraceConfig`, `HttpTraceLayer`, `MakeRequestSpan`, `RequestOutcome`, `trace.*`, per-request span / request summary line | `docs/claude/subsystems.md` |
| `prelude`, `use r2e::prelude::*`, feature flags, `Params`, re-exports, what's available by default | `docs/claude/prelude-features.md` |
| `r2e new`, `r2e dev`, `r2e generate`, `r2e add`, `r2e doctor`, `r2e routes`, CLI templates, scaffolding | `docs/claude/cli.md` |
| `quic`, `quinn`, `h3`, HTTP/3, `serve_h3`, `QuicEndpoint`, `QuicConnection`, `Alt-Svc`, `build_server_config`, raw QUIC streams, `server.quic.*` | `docs/features/18-quic.md` |
| `server.workers`, `per-core`, SO_REUSEPORT, sharded serving, thread-per-core, `parse_workers`, `MAX_WORKERS`, `ServeStrategy`, `rt::spawn`, `spawn_ctl`, `set_control_plane`, control plane / data plane, worker runtimes | `docs/features/19-sharded-serving.md` |
| `#[any]`, `#[fallback]`, catch-all, wildcard `{*path}` routes, proxy/gateway routing, raw `Request` param, streaming responses (`Body::from_stream`), escape-hatch ladder (`merge_router`, `with_layer_fn`) | `docs/features/20-proxy-catch-all.md` |
| `StopHandle`, programmatic stop, graceful shutdown/drain, `on_drain`, `on_stop`, `ServeContext`, `on_serve`, `track`, shutdown token, `shutdown_grace_period`, gRPC drain, readiness flip / LB deregistration | `docs/features/22-serve-lifecycle.md` |
| HTTP metrics, `Prometheus` plugin, `Prometheus::layer_only()`, `prometheus.expose_endpoint`, `MetricsFacade`, `metrics-facade` feature, `HttpMetricsRecorder`, `metrics` crate vs `prometheus` crate, emitted metric names/labels, migrating an app with a hand-written metrics middleware | `docs/claude/metrics-stacks.md` |
| dev-reload config semantics, hot-patch cycles, cached state, partial rebuild, `provided_reuse_clones`, pinned provided values, graph fingerprint scope, `LiveConfigRegistry` identity across cycles, stale typed config under `r2e dev`, `crate::dev` statics | `docs/claude/dev-reload-config-semantics.md` |
| DI/builder refactor status & phases, `.register()`, `build_state()`, HList state, `HasBean`/`BeanLookup`/`BeanAccess`, `FromRequestPartsVia`, `.when()`, `register_controllers`, unified registration, `recursion_limit`, `#[derive(ProvideBundle)]`/`provide_all` | `docs/claude/di-builder-refactor.md` |
| feature modules, `#[module]`, `register_module`, closed subgraph, module imports/exports/encapsulation, modules bringing plugins (`plugins(Type = expr)`) / `requires_plugins`, controllers as beans, `from_context`, `ContextConstruct`, context-as-state, module aggregates (`#[module(modules(..))]`/`register_modules`), path-prefixed modules (`#[module(prefix = "/api/v1")]`) | `docs/claude/di-builder-refactor.md` |
| guards/interceptors as beans, `DecoratorSpec`, `DecoratorBean`, guard compile-time deps, once-at-registration guard construction, `Guard<I>`/`Interceptor<R>` redesign | `docs/claude/guards-interceptors.md` |
| roadmap, backlog, next steps, what to work on, framework gaps, real-app audit (threaty/patina), rejected-design decisions (qualifiers, startup_check) | `docs/claude/roadmap.md` |
| fixing audit findings on hard concurrent code, adversarial verification loop, codex verify passes, persistent fixer agent, mutation checks, merge-verdict exit | `docs/claude/methodology-adversarial-fix-loop.md` |
| EventBus perf/reliability work, distributed backend audit (iggy/kafka/pulsar/rabbitmq), delivery semantics (at-least-once, ack-after-handler), producer batching, `BackendState` dedup/event_id, consume pipelining | `docs/claude/eventbus-perf.md` |
| MCP server, `#[mcp_routes]`, `#[tool]`, `#[resource]`, `#[prompt]`, MCP resources/prompts, `McpRoutes`, `ResourceCall`/`PromptCall`, `IntoResourceResult`/`IntoPromptResult`, `McpServer`, `register_mcp_service`, `Params<T>`, `ToolCall`, `McpError`, `IntoToolResult`, `mcp.*` config, MCP guards/interceptors, streamable HTTP, MCP sessions, `mcp.allowed-hosts`, MCP auth, `mcp.auth.*`, OAuth resource server, PRM (RFC 9728), DCR shim, `McpTokenValidator`, `ScopePolicy`, `#[tool(scopes)]`, `pin_mcp_validator`, `server.public-url` | `docs/features/25-mcp.md` (user guide) + `docs/claude/transport-adapters.md` (adapter internals) |
| new transport / wire adapter, `EndpointDeps`, `endpoint_deps_fold`, `register_grpc_service` compile check, `AppBuilderGrpcExt`, ports-and-adapters shape, per-transport guards decision | `docs/claude/transport-adapters.md` |
| testing DX, `App` trait, `App::setup`/`App::build`, `r2e::launch`, `override_config`, `BootableApp`, `TestApp::boot`, `#[r2e::test(app = ...)]`, `override_bean` (pinned overrides), `override_config_value`, `with_profile`, `application-test.yaml`, `.as_user()`, mocks in tests, dev services / testcontainers | `docs/claude/subsystems.md` (TestApp section); open follow-ups in `docs/claude/roadmap.md` |

**Rules:**
1. Match keywords from your task to the left column. Read **only** the matched file(s).
2. If your task spans two subsystems (e.g., config + beans), read both — but no more.
3. If nothing matches, you probably don't need a reference doc. Proceed with the code.

## Keeping `llm.txt` Fresh

`llm.txt` (hub: golden rules + routing table) + `llm/<topic>.md` (spokes) are the agent-facing reference downstream projects follow **literally**; `llm-full.txt` is generated — never edit it directly. **Any change to a public API surface (traits, macros, builder methods, renames, removals) MUST update the matching topic in the same PR** — a stale example makes consumer agents generate non-compiling code. Every ```rust block must compile against the `r2e` façade (`cargo test -p llm-doctests`; mark deliberately partial snippets `rust,ignore`). After editing, run `scripts/check-llm-docs.sh --update` (recomputes `tokens:` and regenerates `llm-full.txt`), then commit spoke + `llm-full.txt`; CI (`.github/workflows/llm-docs.yml`) fails on drift. Adding a topic also means routing it from `llm.txt` and adding its slug to `TOPICS` in `r2e-cli/src/commands/llm_docs.rs` (`r2e-cli/tests/llm_docs.rs` fails otherwise). Front-matter format and layout decisions: `plans/llm-docs-split.md`.

## Language & Documentation

All documentation, code, comments, and API surfaces are in English.
