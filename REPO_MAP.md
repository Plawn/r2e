# Repository Map

Quick-reference guide to the R2E workspace. Each section lists every file with a one-line description.

> **Dependency flow:** `r2e-http` <- `r2e-macros` <- `r2e-core` <- feature crates (`security`, `events`, `scheduler`, `executor`, `data`, `static`, ...) <- `r2e` (facade) <- `example-*`
>
> **Only `r2e-http` depends on `axum` directly.** All other crates access HTTP types through `r2e_core::http` (which re-exports from `r2e-http`).

---

## Workspace root

```
Cargo.toml              Workspace manifest (all members, patch.crates-io for vendored deps)
Cargo.lock              Dependency lock file
application.yaml        Base configuration (loaded by R2eConfig)
CLAUDE.md               AI coding guidelines and full architecture reference
REPO_MAP.md             This file
README.md               Project README with quick-start and feature overview
LICENSE                  Apache-2.0
CONTRIBUTING.md         Contribution guidelines
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
    mod.rs                  Built-in plugins: Health, AdvancedHealth, Cors, Tracing, DevReload, NormalizePath
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
    layers.rs               Tower layer utilities: default_cors(), init_tracing()
    http_trace.rs           HttpTraceLayer + MakeRequestSpan (per-request span, summary, request id)
    lifecycle.rs            LifecycleController for on_start/on_stop hooks
    service.rs              ServiceComponent trait
    sharded.rs              SO_REUSEPORT sharded serving (server.workers)
    tracing_config.rs       TracingConfig, LogFormat, SpanEvents
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
  lib.rs                    EventBus (subscribe, emit, emit_and_wait), concurrency control

tests/
  event_bus.rs              Emit/subscribe, backpressure, panic isolation, stress tests
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
  async_exec.rs              #[async_exec] codegen returns Result<JoinHandle<T>, RejectedError>
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

---

## r2e-mcp — MCP server

MCP (Model Context Protocol) tools over rmcp's streamable-HTTP transport,
dispatched by R2E (`#[mcp_routes]` + `#[tool]`; guards/interceptors shared
with HTTP).

```
src/
  lib.rs                    AppBuilderMcpExt (register_mcp_service + compile-time dep check), prelude, __macro_support
  plugin.rs                 McpServer plugin: path validation, shared session map, shutdown-token relay, endpoint mount
  config.rs                 McpConfig (`mcp.*`)
  handler.rs                McpRuntime (dispatch table, duplicate-name boot panic) + rmcp ServerHandler impl
  registry.rs               McpServiceRegistry (filled at registration, drained once at router build)
  resource_updates.rs       Injectable resource-update publisher (legacy + current subscriptions)
  uri_template.rs           RFC 6570 reverse matching and captured variables
  service.rs                McpService trait (what #[mcp_routes] implements)
  route.rs                  ToolRoute/ToolCall/ToolAnnotations/ToolInvoke
  params.rs                 Params<T> + sealed ObjectParams + ToolParams (schemars → object inputSchema)
  result.rs                 IntoToolResult (String/()/Json<T> dual encoding/CallToolResult/Result)
  error.rs                  McpError → CallToolResult{isError} or JSON-RPC ErrorData
  guard.rs                  GuardContext bridge: HTTP Guard<I> over transport request parts

tests/
  support/mod.rs            oneshot-driven MCP protocol harness (initialize/session/SSE parsing)
  server/                   plugin, registry, dispatch, schema, interceptors, lifecycle (sharded + stop)
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
  tracing_setup.rs          Tracing subscriber setup
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
tests/
  compile_tests.rs          Trybuild compile-fail/pass UI tests for macros
```

---

## examples/

### example-app — Full-featured demo

Exercises all major features: JWT auth, events, scheduling, WebSockets, SSE, file uploads, mixed auth.

```
src/
  main.rs                   App entry point (AppBuilder with all plugins)
  state.rs                  AppState definition
  services.rs               UserService, NotificationService
  models.rs                 User, Notification models
  db_identity.rs            Custom database-backed Identity impl
  controllers/
    mod.rs                  Controller module exports
    user_controller.rs      CRUD with auth, caching, roles
    account_controller.rs   Account management
    config_controller.rs    Configuration endpoints
    data_controller.rs      Data access demo
    db_identity_controller.rs  Custom identity demo
    event_controller.rs     Event emission demo
    mixed_controller.rs     Mixed public/protected endpoints
    notification_controller.rs  Notification routes
    scheduled_controller.rs Scheduled task demo
    sse_controller.rs       Server-Sent Events demo
    upload_controller.rs    File upload handling
    ws_controller.rs        WebSocket handler
tests/
  app/                      End-to-end boots of the demo app (users, orders, proxy, upload, config, ordering)
  security/                 Guards, OpenFGA, mixed public/protected controllers
  http/                     Verbs, SSE, WS, rate limiting, OpenAPI mapping
  events/                   EventBus consumers (controllers + beans)
  scheduling/               Scheduled tasks (controllers + beans)
  transverse/               Interceptors, lifecycle hooks
```

### example-mcp — MCP server demo

One `CalcService` bean behind two transports: `#[mcp_routes] MathTools` (tools
with guards + interceptors) and an HTTP `CalcController`. `tests/mcp_e2e.rs`
drives the full MCP session dance through `TestApp`.

### example-postgres — Database integration

SQLx + PostgreSQL with migrations.

```
src/
  main.rs                   Postgres app entry point
  state.rs                  State with SqlitePool
  error.rs                  Custom error type
  controllers/article_controller.rs   Article CRUD
  models/article.rs                   Article entity
  services/article_service.rs         Article service
migrations/
  20250101000001_create_articles.sql   Schema migration
docker-compose.yml                     PostgreSQL container
```

### example-multi-tenant — Custom identity & guards

Custom Identity impl, tenant isolation guard.

```
src/
  main.rs                   Multi-tenant app entry point
  state.rs                  State
  tenant_identity.rs        Custom Identity impl for tenants
  tenant_guard.rs           Tenant validation guard
  controllers/tenant_controller.rs    Tenant-scoped routes
  controllers/admin_controller.rs     Admin routes
  models/project.rs                   Tenant project model
  services/project_service.rs         Project service
```

### example-websocket-chat — Real-time chat

WebSocket connections + event consumers.

```
src/
  main.rs                   Chat app entry point
  state.rs                  State with WsRooms
  models.rs                 Chat message model
  controllers/chat_controller.rs      WebSocket handler
  controllers/history_controller.rs   Message history
  controllers/consumer.rs             Event consumer
  services/chat_service.rs            Chat service
```

### example-executor — PoolExecutor + BackgroundService

Demonstrates the `Executor` plugin, `#[async_exec]` returning `Result<JoinHandle<T>, RejectedError>`, and a `#[derive(BackgroundService)]` tick worker that submits detached jobs.

```
src/
  main.rs                   Single-binary demo (POST /reports/:id, GET /metrics, TickWorker)
```

### example-microservice — Multi-service architecture

Two services (order + product) communicating via HTTP.

```
src/
  order/                    Order service (separate binary)
    main.rs, state.rs, models.rs
    controllers/order_controller.rs
    services/order_service.rs, product_client.rs
  product/                  Product service (separate binary)
    main.rs, state.rs, models.rs
    controllers/product_controller.rs
    services/product_service.rs
  shared/
    mod.rs, types.rs        Shared types between services
application-order.yaml      Order service config
application-product.yaml    Product service config
```

---

## vendor/

### openfga-rs — Vendored OpenFGA client

Patched to use tonic ~0.12 with `channel`-only features (avoids axum-core version conflict).

```
vendor/openfga-rs/
  src/lib.rs                OpenFGA gRPC client
  proto/                    Protobuf definitions (openfga, google, validate)
  README.md                 Vendor rationale and patch details
```

The workspace `[patch.crates-io]` section in the root `Cargo.toml` points to this directory.
