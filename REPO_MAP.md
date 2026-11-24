# Repository Map

Quick-reference guide to the R2E workspace. Each section lists every file with a one-line description.

> **Dependency flow:** `r2e-rt` <- `r2e-http` <- `r2e-macros` <- `r2e-core` (`r2e-tracing` also sits at the bottom, consumed first by `r2e-core`) <- feature crates (`security`, `events`, `scheduler`, `executor`, `data`, `static`, `tenant`, `grpc`, `mcp`, ...) <- `r2e` (facade) <- `example-*`
>
> **Only `r2e-http` depends on `axum` directly.** All other crates access HTTP types through `r2e_core::http` (which re-exports from `r2e-http`).

---

## Workspace root

```
Cargo.toml              Workspace manifest (all members)
Cargo.lock              Dependency lock file
application.yaml        Base configuration (loaded by R2eConfig)
CLAUDE.md               AI coding guidelines: golden rules + routing table to docs/claude/
AGENTS.md               Entry point for AGENTS.md-aware agents; delegates to CLAUDE.md
REPO_MAP.md             This file
README.md               Project README with quick-start and feature overview
CHANGELOG.md            Frozen workspace history up to 0.6.0 (per-crate CHANGELOG.md since)
LICENSE                 Apache-2.0
CONTRIBUTING.md         Contribution guidelines
clippy.toml             Clippy configuration (disallowed methods, e.g. the tokio::spawn ban)
rustfmt.toml            rustfmt configuration
rust-toolchain.toml     Pinned toolchain (trybuild .stderr snapshots depend on it)
Dioxus.toml             Subsecond / dx configuration for dev-reload
llm.txt                 Agent-facing reference hub: golden rules + routing table to llm/
llm-full.txt            Generated single-file version of llm.txt + llm/ (never edit by hand)
llm/                    Agent-facing topic spokes (one .md per topic)
llm-doctests/           Compiles every ```rust block of llm/ against the facade
docs/                   mdBook (docs/book), feature guides (docs/features), agent docs
                        (docs/claude), migration guides (docs/migration), CI notes (docs/ci.md)
scripts/                CI and release scripts (boundaries, versions, publish, smoke, llm docs)
tools/                  Benchmark drivers (bench-hotpath.sh, bench-sharded.sh)
plans/                  Design plans and audits (archive/ for finished ones)
misc/                   Historical notes and scratch documents
requests/               Scratch directory for HTTP request files (currently empty)
coverage/               Saved llvm-cov coverage reports
.github/                CI workflows (see docs/ci.md)
```

---

## r2e-rt — Async-runtime facade

Bottom of the graph and the only crate that names `tokio` (re-exported as `r2e_core::rt`). Everything above goes through it — spawn, timers, sync primitives, `task_local!`, runtime ids.

```
src/
  lib.rs                    The facade: re-exports and thin wrappers over tokio / tokio-util / tokio-stream
tests/
  rt/                       One target: runtime_id (RuntimeId), task_local (per-task scoping through the facade)
```

---

## r2e-http — HTTP abstraction layer

Sole owner of the `axum` dependency. Re-exports Router, extractors, responses, middleware, routing, WebSocket, and multipart types.

```
src/
  lib.rs                    Entry point — top-level re-exports (Router, Json, Extension, serve, etc.)
  body.rs                   Body, to_bytes
  extract.rs                Extractors (State, Path, Query, Form, FromRequestParts, etc.)
  header.rs                 HTTP headers, StatusCode, Method, HeaderMap, Parts
  middleware.rs              from_fn, from_fn_with_state, Next
  response.rs               IntoResponse, Response, Html, Redirect, Sse
  routing.rs                get, post, put, patch, delete, Route
  ws.rs                     WebSocket types (feature = "ws")
  multipart.rs              Multipart extractor (feature = "multipart")
```

---

## r2e-tracing — Process-global tracing subscriber

Owns the one-shot global `tracing` subscriber. No tokio, no `r2e-*` dependency (deps: tracing, tracing-core, tracing-subscriber, tracing-log, serde/serde_json); not a dependency of `r2e-http` — `r2e-core` is its first consumer and installs it from `build_state()`.

```
src/
  lib.rs                    Re-exports (TracingOptions, LogFormat, SpanEvents, JsonOptions, BoxedLayer, install, installed, AlreadyInstalled, capture_boot_logs, discard_boot_logs, BootLogs, Suppress, LogFields, console_layer_with_writer, tracing_subscriber)
  options.rs                TracingOptions + builders, LogFormat, SpanEvents (to_fmt_span), JsonOptions
  install.rs                install(opts, layers): Registry + [contributed…, console] + EnvFilter (one shot), installed(), AlreadyInstalled
  boot.rs                   boot-log buffer/forwarder: capture_boot_logs, discard_boot_logs, BootLogs (replay on install, raw flush on drop)
  console.rs                console fmt layer (text / JSON / flat JSON, TTY-aware ansi), console_layer_with_writer
  flat_json.rs              flat JSON line format (`tracing.json.flatten: true`)
  noise.rs                  Suppress layer (Suppress::new / Suppress::error) — gates the console and every sibling layer
  enrich.rs                 LogFields span extension (fields printed on every line inside the span)
tests/
  boot/                     boot forwarder: buffered events replayed through the real stack (own binary)
  boot_bound/               bounded boot buffer + overflow WARN (own binary)
  boot_disabled/            discard_boot_logs, then a later install (own binary)
  boot_flush/               boot fails before install: raw flush of buffered lines (own binary)
  boot_foreign/             foreign subscriber wins: capture inert, unknown winner (own binary)
  boot_rust_log/            RUST_LOG=debug: DEBUG buffered + replayed (own binary)
  boot_rust_log_directives/ directive-scoped RUST_LOG buffer + installed filter at replay (own binary)
  install/                  one-shot install, contributed layers, Suppress gating siblings (own binary)
  install_invalid_filter/   invalid filter falls back to info (own binary)
  format/                   flat JSON + LogFields in text/JSON/flat JSON (scoped subscriber)
  options/                  pure-value tests (format, span_events)
  support/                  shared test helpers
```

---

## r2e-macros — Procedural macros

No runtime dependencies. Generates handlers, extractors, and DI wiring at compile time.

```
src/
  lib.rs                    Entry point — all #[proc_macro_attribute] and #[proc_macro_derive] definitions

  attrs/                    Transforming attribute macros
    bean_attr.rs            #[bean] — auto-detects sync/async, generates Bean or AsyncBean impl
    controller_attr.rs      Entry point for #[controller(...)] (transforming attribute)
    main_attr.rs            #[r2e::main] / #[r2e::test] entry-point wrappers
    module_attr.rs          #[module] — feature modules
    producer_attr.rs        #[producer] — free-function factory, generates Producer impl
    routes_attr.rs          Entry point for #[routes] on impl blocks
    test_suite_attr.rs      #[test_suite] — shared-app test suites

  derives/                  Derive macros
    api_error_derive.rs     #[derive(ApiError)] — typed error responses
    bean_derive.rs          #[derive(Bean)] — field-level #[inject] + #[config]
    bg_service_derive.rs    #[derive(BackgroundService)] — generates ServiceComponent from #[inject]/#[config]
    cacheable_derive.rs     #[derive(Cacheable)] — cache key generation
    config_derive.rs        #[derive(Config)] — typed configuration sections
    decorator_bean_derive.rs #[derive(DecoratorBean)] — guard/interceptor bean specs
    from_config_value_derive.rs #[derive(FromConfigValue)]
    from_multipart.rs       #[derive(FromMultipart)] — multipart form parsing
    params_derive.rs        #[derive(Params)] — request-parameter structs

  parsing/                  Attribute-input parsing -> definition structs
    controller_parsing.rs   Parse the struct -> ControllerStructDef
    routes_parsing.rs       Parse ItemImpl -> RoutesImplDef
    grpc_routes_parsing.rs  Parse #[grpc_routes] impl blocks

  codegen/                  Emission (split by concern)
    controller_codegen.rs   Generate the core, meta module, __R2eRequestData_ extractor, __R2eRequest_ façade (+ Deref), ContextConstruct impl
    controller_impl.rs      Generate impl Controller<State> (route registration, scheduled_tasks)
    handlers.rs             Generate per-route Axum handler functions
    wrapping.rs             Generate interceptor/guard wrapping around method bodies
    decorators.rs           Decorator-set construction (guards/interceptors)
    scheduled.rs            Scheduled-task registration codegen
    transverse.rs           Shared bean-level transverse codegen (#[consumer]/#[scheduled]/#[intercept]/#[post_construct])

  model/                    Shared parsed-definition types
    types.rs                Shared IR types (InjectedField, IdentityField, RequestField, ConfigField, RouteMethod, ...)
    route.rs                HttpMethod enum and RoutePath parser
    field_resolver.rs       Field resolution shared by bean/controller macros
    type_list_gen.rs        Type-level list generation helpers

  util/
    crate_path.rs           Dynamic crate path resolution (r2e vs r2e-core facade detection)
    type_utils.rs           Type helpers (unwrap_option_type, ...)
    hash_tokens.rs          Token hashing (graph fingerprints)
    runtime_args.rs         Runtime-arg parsing for entry-point macros

  extract/                  Attribute extraction helpers
    async_exec.rs           Extract #[async_exec(executor = "...")] definitions
    consumer.rs             Extract #[consumer(bus = "...")] definitions
    duration.rs             Duration literal parsing
    managed.rs              Extract #[managed] parameter annotations
    plugins.rs              Plugin-related attribute extraction
    route.rs                Extract #[get], #[post], #[roles], #[guard], #[intercept], ...
    scheduled.rs            Extract #[scheduled(every = ..., cron = ...)] definitions

  grpc_codegen/             Tonic service wiring (trait_impl, service_impl)
```

---

## r2e-core — Runtime foundation

AppBuilder, controllers, guards, interceptors, plugins, configuration, DI, and HTTP utilities.

```
src/
  lib.rs                    Entry point — root type re-exports (module paths are the API: r2e_core::<group>::<module>)
  controller.rs             Controller<S, W>, ContextConstruct and EndpointDeps trait definitions
  error.rs                  HttpError enum (BadRequest, NotFound, Unauthorized, Forbidden, Internal)
  rt.rs                     Task placement facade (rt::spawn, spawn_ctl, control plane vs workers)
  state.rs                  R2eState wrapper type
  type_list.rs              Heterogeneous type list (TNil, TCons, Contains, AllSatisfied) for compile-time DI
  types.rs                  Shared type definitions
  prelude.rs                Convenience re-exports

  beans/                    DI system: Bean, AsyncBean, Producer, BeanContext, BeanRegistry, graph resolve
  builder/                  AppBuilder fluent API (provide, register, when, build_state, register_controller(s), serve)
  builtins/
    mod.rs                  Built-in plugins: Health, AdvancedHealth, DevReload, NormalizePath; hidden compile-error shims for the removed Tracing / ConfiguredTracing plugins
    cors.rs                 Cors plugin + CorsConfig (`cors.*`): origin grammar validated at boot, builder > file > default_origin, fail-closed outside dev
    health.rs               HealthIndicator trait, HealthBuilder, HealthState, /health endpoints
    http_trace.rs           HttpTrace plugin + HttpTraceBuilder + HttpTraceConfig (`trace.*`)
    request_id.rs           RequestId extractor and RequestIdPlugin
    secure_headers.rs       SecureHeaders plugin + builder (CSP, HSTS, X-Frame-Options, ...)
  config/
    mod.rs                  R2eConfig, ConfigValue, FromConfigValue, ConfigError — public API
    loader.rs               YAML file loader (application.yaml + .env + env vars)
    registry.rs             Config section registry (register_section, validate_section)
    runtime.rs              LiveConfig runtime registry
    secrets.rs              SecretResolver trait, DefaultSecretResolver (env var interpolation)
    typed.rs                Typed config value extraction
    validation.rs           Config key validation
    value.rs                ConfigValue enum (String, Int, Float, Bool, List, Map)
  decorators/
    decorator.rs            DecoratorSpec contract, DecoSlot, SelfBuilt
    guards.rs               Guard<S,I>, PreAuthGuard<S>, GuardContext, RolesGuard, PathParams
    interceptors.rs         Interceptor<R> trait, InterceptorContext, Cacheable trait
  di/
    event_subscriber.rs     EventSubscriber trait (beans with #[consumer] methods)
    late.rs                 Late<T> deferred plugin provisions
    lazy.rs                 Lazy<T> beans
    meta.rs                 MetaRegistry for collecting route metadata (used by OpenAPI)
    module.rs               FeatureModule (closed subgraph modules)
    scheduled_source.rs     ScheduledSource trait (beans with #[scheduled] methods)
  http/
    mod.rs                  HTTP module — re-exports from r2e-http (Router, StatusCode, HeaderMap, ...)
    ws.rs                   WebSocket re-exports + IsWebSocket trait
  plugin/                   Plugin machinery: Plugin, Plugin, DeferredAction, contexts, graph handle
  runtime/
    dev.rs                  Dev-reload statics + endpoints (feature = "dev-reload" consumers)
    layers.rs               Tower layer utilities: default_cors(), catch-panic, fallbacks
    tracing.rs              install_tracing (called by build_state: `tracing:` section + contributed layers → r2e_tracing::install) + compile-error shims for the removed init functions
    http_trace.rs           HttpTraceLayer + MakeRequestSpan (per-request span, summary, request id)
    lifecycle.rs            LifecycleController for on_start/on_stop hooks
    service.rs              ServiceComponent trait
    sharded.rs              SO_REUSEPORT sharded serving (server.workers)
    tracing_config.rs       TracingConfig + JsonLogConfig (yaml `tracing:`) → TracingOptions; FromConfigValue for LogFormat/SpanEvents
  web/
    extract.rs              FromRequestPartsVia, ViaBean/ViaAxum, BeanExtract, PeerAddr
    managed.rs              ManagedResource<S> trait, ManagedErr<E> wrapper
    multipart.rs            Multipart extraction (feature = "multipart")
    pagination.rs           Page/Pageable
    params.rs               Params derive helpers (ParamError, parse_query_string)
    request_head.rs         RequestHead
    sse.rs                  SseBroadcaster, SseStream for Server-Sent Events
    validation.rs           Automatic validation via garde (autoref specialization)
    ws.rs                   WsStream, WsHandler, WsBroadcaster, WsRooms (feature = "ws")

tests/                      One directory target per subsystem (support/ = shared helpers)
  builder/  config/  controller/  decorators/  di/  http/  plugin/  runtime/
```

---

## r2e-security — JWT & OIDC

JWT validation, JWKS caching, AuthenticatedUser extractor, role extraction.

```
src/
  lib.rs                    Entry point — re-exports, __macro_support module
  config.rs                 SecurityConfig (issuer, audience, JWKS URL, static keys)
  error.rs                  SecurityError enum (MissingAuthHeader, InvalidToken, ...)
  extractor.rs              AuthenticatedUser FromRequestParts impl, extract_bearer_token, extract_jwt_claims
  identity.rs               AuthenticatedUser, FromValidatedJwtClaims, IdentityBuilder, extractor macro
  jwt.rs                    JwtClaimSet, JwtClaimsValidator, JwtValidator — typed token validation
  jwks.rs                   JwksCache — background JWKS key refresh
  keycloak.rs               RealmRoleExtractor, ClientRoleExtractor for Keycloak
  openid.rs                 StandardRoleExtractor, Composite, Merge — pluggable role extraction

tests/
  config.rs                 SecurityConfig tests
  error.rs                  SecurityError -> HTTP response tests
  extractor.rs              Bearer token extraction tests
  identity.rs               AuthenticatedUser construction and Identity trait tests
  jwt.rs                    JWT validation tests (valid, expired, wrong key, ...)
  keycloak.rs               Keycloak role extraction tests
  openid.rs                 OpenID role extraction tests
```

---

## r2e-events — Event bus

In-process typed pub/sub with backpressure.

```
src/
  lib.rs                    EventBus trait, subscriptions, EventMetadata, retry policy, re-exports
  local.rs                  LocalEventBus — the in-process implementation (emit, emit_and_wait, request-reply)
  sse_bridge.rs             Bridge events to an SSE broadcaster
  backend/                  Shared plumbing for the distributed backends: dispatch, metadata codec,
                            pending replies, reconnect, BackendState (dedup / event_id), topic, watermark

tests/
  event_bus.rs              Emit/subscribe, backpressure, panic isolation, stress tests
  backend_state.rs, backend_watermark.rs, event_id.rs, pending.rs, reconnect.rs, sse_bridge.rs
```

### Distributed backends — `r2e-events/backends/{iggy,kafka,pulsar,rabbitmq}`

Crates `r2e-events-iggy`, `r2e-events-kafka`, `r2e-events-pulsar`, `r2e-events-rabbitmq` (facade features `events-iggy`, `events-kafka`, `events-pulsar`, `events-rabbitmq`). Each one implements `EventBus` over its broker with the same layout:

```
src/
  lib.rs                    Entry point and re-exports
  builder.rs                Builder for the bus (connection, topics, consumer group)
  bus.rs                    The EventBus implementation
  config.rs                 Connection / security settings
  error.rs                  Broker error -> EventBusError mapping
  inner.rs                  Shared inner state behind an Arc
  (kafka) request.rs        Request-reply transport (reply topics)
  (iggy) dispatch.rs, poller.rs, topic.rs   Consumer polling and topic management
tests/
  <backend>_event_bus.rs    Integration tests against a real broker
```

---

## r2e-scheduler — Background tasks

Interval, cron, and delayed task scheduling with graceful shutdown.

```
src/
  lib.rs                    Scheduler Plugin, SchedulerHandle, task runner loop
  types.rs                  ScheduleConfig, ScheduledTaskDef<T>, ScheduledTask trait, ScheduledResult

tests/
  scheduler/                One target, one module per concern (core, handle, overlap,
                            skip_if, runtime_control, dynamic, plugin, plugin_config,
                            serve_lifecycle, sharded, duration, types)
  driver_edge_test/         Driver edge cases (min-heap ordering, drift, drain races)
```

---

## r2e-executor — Managed task pool

Bounded `PoolExecutor` (semaphore concurrency + mpsc-style queue cap), graceful drain. Powers `#[async_exec]` and `#[derive(BackgroundService)]`.

```
src/
  lib.rs                    ExecutorConfig, PoolExecutor, ExecutorMetrics, RejectedError, Executor Plugin

tests/
  executor.rs               submit/await, concurrency cap, queue rejection, graceful + abort shutdown
  bg_service.rs             #[derive(BackgroundService)] roundtrip
  async_exec.rs             #[async_exec] codegen returns Result<JobHandle<T>, RejectedError>
```

---

## r2e-data-sqlx — Managed SQLx transactions

```
src/
  lib.rs                    Entry point
  tx.rs                     Cancellation-safe Tx<'a, DB>
```

---

## r2e-data-diesel — Managed Diesel transactions

```
src/
  lib.rs                    Entry point
  lib.rs                    DieselTx<C>, blocking-pool execution, lifecycle
```

---

## r2e-cache — TTL cache

Thread-safe cache with pluggable backends.

```
src/
  lib.rs                    TtlCache<K,V>, CacheStore trait, InMemoryStore, global singleton

tests/
  ttl_cache.rs              Cache insert/get/expire, CacheStore backend tests
```

---

## r2e-rate-limit — Rate limiting

Token-bucket algorithm with pluggable backends.

```
src/
  lib.rs                    RateLimiter<K>, RateLimitBackend, InMemoryRateLimiter, RateLimitRegistry
  guard.rs                  RateLimit builder, RateLimitGuard, PreAuthRateLimitGuard

tests/
  rate_limiter.rs           Token-bucket algorithm and registry tests
```

---

## r2e-openapi — API documentation

OpenAPI 3.1.0 spec generation from route metadata.

```
src/
  lib.rs                    Entry point
  builder.rs                OpenApiConfig, OpenApiBuilder
  ext.rs                    AppBuilderOpenApiExt extension trait
  handlers.rs               /openapi.json and /docs endpoint handlers
  schema.rs                 SchemaRegistry, SchemaProvider for JSON Schema generation
```

---

## r2e-openfga — OpenFGA authorization

Relation-based access control via OpenFGA.

```
src/
  lib.rs                    Entry point — re-exports, MockBackend
  backend.rs                OpenFGA backend client (gRPC)
  cache.rs                  DecisionCache for caching authorization decisions
  config.rs                 OpenFgaConfig
  error.rs                  OpenFgaError enum
  guard.rs                  FgaCheck builder, FgaGuard (resolves object from path/query/header)
  registry.rs               OpenFgaRegistry (check, invalidate, cache management)

tests/
  backend.rs                MockBackend tests
  cache.rs                  DecisionCache TTL and eviction tests
  guard.rs                  FgaGuard object resolution and security tests
  registry.rs               Registry check/cache integration tests
```

### r2e-openfga-model — `r2e-openfga/model`

```
src/
  lib.rs                    Parser for the OpenFGA DSL (schema 1.1), re-exports
  model.rs                  Data model of an authorization model
  parser.rs                 Hand-rolled DSL parser
  validate.rs               Semantic validation of a parsed AuthorizationModel
tests/
  parser.rs                 DSL -> JSON round-trips against the vendored openfga/language transformer
  validate.rs               Semantic validation
```

### r2e-openfga-macros — `r2e-openfga/macros`

```
src/
  lib.rs                    Proc macros for r2e-openfga
  model_macro.rs            Expansion of model!(pub mod authz = "fga/model.fga")
```

---

## r2e-mcp — MCP server

MCP (Model Context Protocol) tools over rmcp's streamable-HTTP transport,
dispatched by R2E (`#[routes]` + `#[tool]`; one type = one port, MCP members
live on their own type next to the HTTP controller; guards/interceptors shared
with HTTP).

```
src/
  lib.rs                    AppBuilderMcpExt (register_mcp_service + compile-time dep check), prelude, __macro_support
  plugin.rs                 McpServer plugin: path validation, shared session map, shutdown-token relay, endpoint mount
  config.rs                 McpConfig (`mcp.*`)
  catalog/                  Catalog (boot-time member table, groups, duplicate-name boot panic) + copy-on-write SessionView
    mod.rs                  Catalog::build/view + key checks, CatalogOptions, ServerIdentity, principal_in
    family.rs               Family/TemplateFamily dispatch tables, FamilyInputs, enabled-member folding
    view.rs                 SessionView, PrivateMembers, Changed, catalog Member/GroupDef/Members
    validate.rs             scope-vs-auth and completion-provider checks (boot + runtime)
  handler.rs                rmcp ServerHandler impl: per-session dispatch, session↔principal binding, list_changed delivery
  session/                  McpSession handle, McpSessions bean, McpSessionInit hook, SessionToolset
    mod.rs                  module docs + re-exports (every r2e_mcp::session::* path)
    error.rs                McpMemberError (+ McpSessionError alias)
    id.rs                   SessionId, SESSION_ID_HEADER, track_session_id, SessionLink, SessionBindings
    init.rs                 SessionToolset, SessionInit, McpSessionInit, erased init hook
    state.rs                SessionState (per-session view, member resolution, copy-on-write update), Draft
    handle.rs               McpSession
    sessions.rs             McpSessions bean
  dynamic.rs                DynamicTool/DynamicResource/DynamicPrompt builders (session-private members)
  registry.rs               McpServiceRegistry (filled at registration, drained once at router build)
  resource_updates.rs       Injectable resource-update publisher (legacy + current subscriptions)
  uri_template.rs           RFC 6570 reverse matching and captured variables
  service.rs                McpService trait (what #[routes] implements for MCP members)
  route.rs                  ToolRoute/ToolCall/ToolAnnotations/ToolInvoke
  params.rs                 Params<T> + sealed ObjectParams + ToolParams (schemars → object inputSchema)
  result.rs                 IntoToolResult (String/()/Json<T> dual encoding/CallToolResult/Result)
  error.rs                  McpError → CallToolResult{isError} or JSON-RPC ErrorData
  guard.rs                  GuardContext bridge: HTTP Guard<I> over transport request parts

tests/
  support/mod.rs            oneshot-driven MCP protocol harness (initialize/session/SSE parsing)
  server/                   plugin, registry, dispatch, schema, interceptors, lifecycle (sharded + stop), dynamic (per-session lists)
  auth/                     OAuth resource server, scopes/roles, session binding (sessions)
```

---

## r2e-grpc — gRPC server

Tonic gRPC on a separate port or multiplexed on the HTTP port, reflection, grpc-web; guards and identity reuse the HTTP contracts.

```
src/
  lib.rs                    GrpcServer plugin, re-exports
  server.rs                 Transport mode (separate port / multiplexed)
  service.rs                Trait for gRPC services registered with the builder
  registry.rs               Accumulates gRPC services during registration
  module.rs                 Feature-module gRPC services — #[module(grpc_services(...))]
  multiplex.rs              Routes a request to gRPC, grpc-web or HTTP by content type
  head.rs                   Per-call request head: a gRPC call seen as HTTP Parts
  identity.rs               Identity helpers for gRPC calls
  status.rs                 Rejection -> tonic::Status projection
tests/
  guard.rs, head.rs, identity.rs, multiplex.rs, reflection.rs, registry.rs
```

### r2e-grpc-build — `r2e-grpc/build`

```
src/
  lib.rs                    build.rs helper: compile() the crate's .proto files (paired with include_protos!)
tests/
  lib.rs
```

---

## r2e-oidc — Embedded OAuth/JWT issuer

`OidcServer` plugin: RS256 issuer with Authorization Code + PKCE, client_credentials and a dev-only password grant (no ID tokens, no federation).

```
src/
  lib.rs                    OidcServer plugin, re-exports
  config.rs                 Issuer configuration
  handlers.rs               Discovery, JWKS, authorize, token, userinfo endpoints
  authorization.rs          One-time Authorization Code + PKCE state
  client.rs                 Confidential (client_credentials) clients
  store.rs                  User store trait and results
  token.rs                  JWT signing service
  keys.rs                   RSA key pair, JWKS publication
  state.rs                  Shared handler state
  error.rs                  OAuth 2.0 error responses (RFC 6749 §5.2)
tests/
  config.rs, discovery.rs, hot_reload.rs, integration.rs, jwks.rs, token_endpoint.rs, userinfo.rs
```

---

## r2e-tenant — Multi-tenant bean routing

`Tenant<T>` / `TenantId` extractors, `Tenanted<T>` per-tenant resource map, `Tenancy` and `PerTenant` plugins, `tenancy.*` config.

```
src/
  lib.rs                    Entry point and re-exports
  id.rs                     Validated TenantId
  config.rs                 The tenancy.* config section
  error.rs                  TenantError and its HTTP mapping
  extract.rs                Tenant<T> and TenantId extractors
  resolver.rs               SPI #1 — request -> TenantId (TenantResolver, SyncTenantResolver)
  source.rs                 SPI #2 — building one tenant's resource (TenantSource), TenantContext cascade
  router.rs                 TenantRouter: one bean serving every per-tenant extractor
  plugin.rs                 Tenancy (install once) and PerTenant<T> (one per resource type)
  map/                      Tenanted<T>: api, resolve (single flight), eviction, negative cache,
                            dispose, drain, epoch, metrics, settings, state, test_seams
tests/
  tenant/                   One target: cascade, error, extractor, id, map, plugin, resolver (+ fixtures)
```

---

## r2e-devtools — Hot reload

```
src/
  lib.rs                    Subsecond hot-reload integration (feature dev-reload, used by `r2e dev`)
```

---

## r2e-devservices — Testcontainers dev services

Exempt from the tokio boundary by design.

```
src/
  lib.rs                    Entry point and re-exports
  service.rs                DevService: how to start a containerized dev service
  postgres.rs               DevPostgres
  redis.rs                  DevRedis
  keycloak.rs               DevKeycloak
  openfga.rs                OpenFGA dev service
  common.rs                 Shared settings (Ryuk / GC toggles, shared-container fingerprint)
  ryuk.rs                   Process-wide TCP lease so the reaper cleans up killed processes
tests/
  dev_services.rs           Container smoke tests
  openfga.rs                OpenFGA dev-service smoke test
```

---

## r2e-utils — Built-in interceptors

```
src/
  lib.rs                    Entry point — re-exports
  interceptors.rs           Logged, Timed, Cache, CacheInvalidate, Counted, MetricTimed

tests/
  interceptors.rs           Interceptor behavior tests
```

---

## r2e-test — Test utilities

```
src/
  lib.rs                    Entry point — re-exports
  app.rs                    TestApp (in-process HTTP client), TestRequest, TestResponse, JSON-path assertions
  jwt.rs                    TestJwt builder (generates valid JWTs for tests)

tests/
  app.rs                    JSON-path resolution and TestResponse tests
```

---

## r2e-observability — Tracing & telemetry

```
src/
  lib.rs                    Entry point
  config.rs                 Observability configuration
  span.rs                   OtelRequestSpan: the OTel span shape for r2e-core's HttpTraceLayer
  propagation.rs            OpenTelemetry context propagation
  tracing_setup.rs          OTel provider init (init_tracing), otel_layer + OtelIds (contributed via ctx.tracing_layer)
```

---

## r2e-prometheus — Metrics

```
src/
  lib.rs                    Entry point, Prometheus plugin
  handler.rs                /metrics endpoint handler
  layer.rs                  Prometheus metrics Tower layer
  metrics.rs                Metric definitions and collectors
```

---

## r2e — Facade crate

```
src/
  lib.rs                    pub use r2e_core::*; feature-gated re-exports of all sub-crates
```

---

## r2e-static — Embedded static files

Embedded static file serving with SPA support. Wraps `rust_embed` with caching, MIME detection, and SPA fallback.

```
src/
  lib.rs                    FileServer trait, EmbedAdapter, EmbeddedFrontend plugin + builder, handler logic

tests/
  embedded.rs               Static file serving tests (exact match, SPA fallback, base path, cache headers)
  fixtures/                 Test HTML, CSS, and JS files
```

---

## r2e-cli — CLI tool

```
src/
  main.rs                   Clap CLI entry point (new, add, dev, generate, doctor, routes)
  commands/
    mod.rs                  Command module re-exports
    new_project.rs          r2e new <name> — project scaffolding with feature selection
    add.rs                  r2e add <ext> — add sub-crate dependency
    dev.rs                  r2e dev — cargo-watch dev server
    generate.rs             r2e generate controller|service|crud|middleware — code generation
    doctor.rs               r2e doctor — project health diagnostics (8 checks)
    routes.rs               r2e routes — static route listing from source
    templates/
      mod.rs                Template utilities (to_snake_case, to_pascal_case, pluralize, render)
      project.rs            Project scaffolding templates
      middleware.rs          Middleware generation template
```

---

## r2e-compile-tests — Macro UI tests

```
src/
  lib.rs                    Test library setup
cases/                      Trybuild cases (pass / fail, with .stderr snapshots)
tests/
  compile_tests.rs          Trybuild compile-fail/pass UI tests for macros
  test_suite_runtime.rs     Runtime checks for #[r2e::test_suite]
```

---

## r2e-edition2024-tests — Edition 2024 canary

`publish = false`. Built with edition 2024 to check generated handlers there.

```
src/
  lib.rs                    Empty on purpose — the assertions live in tests/
tests/
  precise_capture.rs        Handlers returning impl Trait compile under edition 2024
```

---

## r2e-featureless-tests — `default-features = false` canary

`publish = false`. A "shared worker crate" built against `r2e` with `default-features = false`.

```
src/
  lib.rs                    The featureless crate under test
tests/
  service_gate.rs           Runtime half of the canary: #[service(enabled = "…")] gating
```

---

## llm-doctests — Doctests for llm/

`publish = false`. Hosts the ```rust blocks of the agent-facing reference under `llm/` as doctests (`cargo test -p llm-doctests`).

```
build.rs                    Turns each llm/<topic>.md into a module doc (and collects the fixtures)
src/
  lib.rs                    include!s the generated topic modules
  fixtures/<topic>.rs       Placeholder services/apps a topic's snippets name without defining
```

---

## examples/

Every example except `example-microservice` (two binaries) and `example-worker-udp` (one `main.rs`) implements the `App` trait in `src/app.rs` (included by `lib.rs` so tests boot the same type), and its `main.rs` is `r2e::app_main!(...)`.

### example-app — Full-featured demo

Exercises most features: feature modules, JWT auth, events, scheduling, executor, WebSockets, SSE, uploads, OpenFGA, error envelopes, proxy routing, mixed auth. Serves on port 3001 (`application.yaml`).

```
src/
  app.rs                    ExampleApp (App trait): plugins, beans, UserModule / OrderModule (#[module])
  lib.rs                    include!("app.rs") so integration tests boot ExampleApp
  main.rs                   r2e::app_main!(ExampleApp)
  env.rs                    Cold, process-lifetime resources (App::setup)
  error.rs                  Application error type (#[derive(ApiError)])
  services.rs               UserService, OrderService, NotificationService, …
  models.rs                 User, Order, Notification models
  db_identity.rs            Custom database-backed Identity impl
  bin/
    boot_failure.rs         Deliberately failing boot (driven by tests/app/boot_failure.rs)
    boot_failure_graph.rs   Boot failing inside App::build, while the graph is built
  controllers/
    mod.rs                  Controller module exports
    user_controller.rs      CRUD with auth, caching, roles
    order_controller.rs     Controller of the "orders" feature module
    account_controller.rs   Account management
    config_controller.rs    Configuration endpoints
    data_controller.rs      Data access demo
    db_identity_controller.rs  Custom identity demo
    document_controller.rs  OpenFGA fine-grained authorization
    event_controller.rs     Event emission + #[consumer] handlers
    mixed_controller.rs     Mixed public/protected endpoints
    notification_controller.rs  Notification routes
    problem_controller.rs   Error envelopes (error projection)
    proxy_controller.rs     #[any("/proxy/{*path}")] + #[fallback]
    report_controller.rs    Fail-closed auth with #[anonymous] opt-out
    scheduled_controller.rs Scheduled task demo
    sse_controller.rs       Server-Sent Events demo
    upload_controller.rs    File upload handling
    ws_controller.rs        WebSocket handler
application.yaml            App config (port 3001)
application-test.yaml       Test profile
Dioxus.toml                 dev-reload configuration
tests/
  app/                      End-to-end boots of the demo app (users, orders, proxy, upload, config, ordering, boot failure, rollback)
  security/                 Guards, OpenFGA, mixed public/protected controllers
  http/                     Verbs, SSE, WS, rate limiting, error envelope, OpenAPI mapping
  events/                   EventBus consumers (controllers + beans)
  scheduling/               Scheduled tasks (controllers + beans, skip_if, from method, manual tick)
  transverse/               Interceptors, lifecycle hooks
  hotpath/                  Per-request hot-path checks (state, layers, decorators, JWT, accounting)
```

### example-executor — PoolExecutor + BackgroundService

Demonstrates the `Executor` plugin, `#[async_exec]` returning `Result<JobHandle<T>, RejectedError>`, and a `#[derive(BackgroundService)]` tick worker that submits detached jobs.

```
src/
  app.rs                    ExecutorApp, ReportController (POST/GET /reports/:id, GET /metrics), TickWorker
  lib.rs, main.rs           include!("app.rs") / r2e::app_main!(ExecutorApp)
```

### example-grpc — HTTP + gRPC

```
build.rs                    r2e_grpc_build::compile()
proto/
  greeter.proto             Greeter service (SayHello, SayHelloAdmin)
  ticker.proto              Streaming service used by the tests
src/
  app.rs                    GrpcApp, GreeterService (#[grpc_routes]), LogCalls interceptor, HealthController
  lib.rs, main.rs           include!("app.rs") / r2e::app_main!(GrpcApp)
tests/
  grpc_guards.rs, grpc_intercept.rs, grpc_module.rs, grpc_reflection.rs,
  grpc_serve.rs, grpc_test_client.rs, grpc_web.rs (+ common/)
```

### example-mcp — MCP server demo

One `CalcService` bean behind two adapters (one type per port): `CalcController`
serves HTTP (`GET /api/calc/add/{a}/{b}`), `MathTools` serves MCP tools with
guards + interceptors, and `ScientificTools` is an opt-in group.
`tests/mcp_e2e.rs` drives the full MCP session dance through `TestApp`.

```
src/
  app.rs                    McpApp, CalcService, MathTools, ScientificTools, CalcController, ApiKeyGuard
  lib.rs, main.rs           include!("app.rs") / r2e::app_main!(McpApp)
tests/
  mcp_e2e.rs                End-to-end MCP session through TestApp
```

### example-mcp-oauth — Authenticated MCP server

One `whoami` tool behind an OAuth 2.1 resource server configured entirely in `application.yaml` (`mcp.auth.issuer` + `server.public-url`).

```
src/
  app.rs, lib.rs, main.rs
application.yaml            mcp.auth.*, server.public-url, PORT (default 3333)
```

### example-oidc — Embedded issuer

```
src/
  app.rs                    OidcApp, GreetingController (/public, /me, /admin), demo users and client
  lib.rs, main.rs           include!("app.rs") / r2e::app_main!(OidcApp)
```

### example-openfga — Schema-first OpenFGA

```
fga/model.fga               Authorization model (OpenFGA DSL), compiled by model!
src/
  app.rs                    App, model!(pub mod authz = "fga/model.fga")
  controllers/document_controller.rs   Typed FgaCheck guards
tests/
  openfga_test.rs           Against a real OpenFGA server (dev service)
application.yaml            openfga.* config
docker-compose.yml          Local OpenFGA server
```

### example-postgres — Database integration

SQLx + PostgreSQL with migrations through the `SqlxDataSource` plugin.

```
src/
  app.rs                    App, migrations compiled into the binary
  lib.rs, main.rs
  error.rs                  Custom error type
  controllers/article_controller.rs   Article CRUD
  models/article.rs                   Article entity
  services/article_service.rs         Article service
migrations/                 Schema migrations
tests/
  postgres_test.rs          Against a real PostgreSQL (dev service)
application.yaml            datasource.* config
docker-compose.yml          PostgreSQL container
```

### example-multi-tenant — Row-level tenancy, custom identity & guards

Custom Identity impl, tenant isolation guard, one shared database.

```
src/
  app.rs, lib.rs, main.rs
  tenant_identity.rs        Custom Identity impl for tenants
  tenant_guard.rs           Tenant validation guard
  controllers/tenant_controller.rs    Tenant-scoped routes
  controllers/admin_controller.rs     Admin routes
  models/project.rs                   Tenant project model
  services/project_service.rs         Project service
```

### example-multi-tenant-db — Database per tenant

The `tenant` feature: `Tenant<T>`, `Tenanted<T>`, `TenantTx`, one SQLite database per tenant.

```
src/
  app.rs, lib.rs, main.rs
  directory.rs              The master directory: which tenants exist
  tenancy.rs                The two SPIs: resolver and per-tenant source
  controllers.rs            The four ways a route touches a tenant
tests/
  tenancy_test.rs
application.yaml            tenancy.* config
```

### example-websocket-chat — Real-time chat

WebSocket connections + event consumers.

```
src/
  app.rs, lib.rs, main.rs
  models.rs                 Chat message model
  controllers/chat_controller.rs      WebSocket handler
  controllers/history_controller.rs   Message history
  controllers/consumer.rs             Event consumer
  services/chat_service.rs            Chat service
```

### example-sharded-bench — Sharded serving benchmark target

```
src/
  app.rs                    BenchApp, file-backed SQLite pool seeded at boot
  controllers.rs            /plain, /json, /db
  lib.rs, main.rs
application.yaml            server.* (127.0.0.1:3000); workers via R2E_SERVER_WORKERS
```

Driven by `tools/bench-sharded.sh`.

### example-worker-udp — Per-worker services

Runs under `server.workers: per-core`: a UDP echo per worker, mailboxes, worker health and a control-plane aggregator.

```
src/
  main.rs                   Whole demo in one file: #[r2e::main] + r2e::launch!(UdpApp)
application.yaml            server.workers: per-core (required)
```

### example-microservice — Multi-service architecture

Two services (order + product) communicating via HTTP, two binaries.

```
src/
  lib.rs
  order/                    Order service (separate binary)
    main.rs, mod.rs, models.rs
    controllers/order_controller.rs
    services/order_service.rs, product_client.rs
  product/                  Product service (separate binary)
    main.rs, mod.rs, models.rs
    controllers/product_controller.rs
    services/product_service.rs
  shared/
    mod.rs, types.rs        Shared types between services
application-order.yaml      Order service config
application-product.yaml    Product service config
```

---

## r2e-openfga/proto + codegen

### The OpenFGA gRPC client, generated and committed

```
r2e-openfga/proto/          Protobuf definitions (openfga, google, validate, openapiv2)
r2e-openfga/src/proto.rs    Module doc + `include!` of the generated file
r2e-openfga/src/proto/      openfga.v1.rs — generated, committed, client-only
r2e-openfga/codegen/        The generator (publish = false); the only protoc consumer
scripts/generate-openfga-proto.sh   Regenerate, or `--check` for CI drift
```

`r2e-openfga` has no `build.rs` on purpose: one would make `protoc` a hard build
requirement for every consumer enabling the `openfga` feature. Committing the
output keeps the crate buildable with nothing but cargo, and the CI drift check
(`.github/workflows/boundaries.yml`) keeps it honest.
