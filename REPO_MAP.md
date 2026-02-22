# Repository Map

Quick-reference guide to the R2E workspace. Each section lists every file with a one-line description.

> **Dependency flow:** `r2e-macros` <- `r2e-core` <- feature crates (`security`, `events`, `scheduler`, `data`, ...) <- `r2e` (facade) <- `example-*`

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

## r2e-macros — Procedural macros

No runtime dependencies. Generates Axum handlers, extractors, and DI wiring at compile time.

```
src/
  lib.rs                    Entry point — all #[proc_macro_attribute] and #[proc_macro_derive] definitions
  types.rs                  Shared IR types (InjectedField, IdentityField, ConfigField, RouteMethod, ...)
  crate_path.rs             Dynamic crate path resolution (r2e vs r2e-core facade detection)
  route.rs                  HttpMethod enum and RoutePath parser

  # Controller derive pipeline
  derive_controller.rs      Entry point for #[derive(Controller)]
  derive_parsing.rs         Parse DeriveInput -> ControllerStructDef
  derive_codegen.rs         Generate meta module, __R2eExtract_ struct, StatefulConstruct impl

  # Routes attribute pipeline
  routes_attr.rs            Entry point for #[routes] on impl blocks
  routes_parsing.rs         Parse ItemImpl -> RoutesImplDef

  # Routes codegen (split by concern)
  codegen/
    mod.rs                  Codegen module re-exports
    controller_impl.rs      Generate impl Controller<State> (route registration, scheduled_tasks)
    handlers.rs             Generate per-route Axum handler functions
    wrapping.rs             Generate interceptor/guard wrapping around method bodies

  # Attribute extraction helpers
  extract/
    mod.rs                  Extract module re-exports
    consumer.rs             Extract #[consumer(bus = "...")] definitions
    managed.rs              Extract #[managed] parameter annotations
    route.rs                Extract #[get], #[post], #[roles], #[guard], #[intercept], ...
    scheduled.rs            Extract #[scheduled(every = ..., cron = ...)] definitions

  # Bean / Producer macros
  bean_attr.rs              #[bean] — auto-detects sync/async, generates Bean or AsyncBean impl
  bean_derive.rs            #[derive(Bean)] — field-level #[inject] + #[config]
  bean_state_derive.rs      #[derive(BeanState)] — generates FromRef impls for state structs
  producer_attr.rs          #[producer] — free-function factory, generates Producer impl

  # Other derive macros
  cacheable_derive.rs       #[derive(Cacheable)] — cache key generation
  config_derive.rs          #[derive(Config)] — typed configuration sections
  from_multipart.rs         #[derive(FromMultipart)] — multipart form parsing
```

---

## r2e-core — Runtime foundation

AppBuilder, controllers, guards, interceptors, plugins, configuration, DI, and HTTP utilities.

```
src/
  lib.rs                    Entry point — re-exports all public types
  builder.rs                AppBuilder fluent API (provide, with_bean, build_state, register_controller, serve)
  controller.rs             Controller<S> and StatefulConstruct<S> trait definitions
  beans.rs                  DI system: Bean, AsyncBean, Producer, BeanContext, BeanRegistry
  error.rs                  HttpError enum (BadRequest, NotFound, Unauthorized, Forbidden, Internal)
  guards.rs                 Guard<S,I>, PreAuthGuard<S>, GuardContext, RolesGuard, PathParams
  interceptors.rs           Interceptor<R> trait, InterceptorContext, Cacheable trait
  plugin.rs                 Plugin, PreStatePlugin, DeferredAction, DeferredContext
  plugins.rs                Built-in plugins: Health, AdvancedHealth, Cors, Tracing, ErrorHandling, ...
  layers.rs                 Tower layer utilities: default_cors(), default_trace(), init_tracing()
  lifecycle.rs              LifecycleController for on_start/on_stop hooks
  managed.rs                ManagedResource<S> trait, ManagedErr<E>, ManagedError wrappers
  meta.rs                   MetaRegistry for collecting route metadata (used by OpenAPI)
  request_id.rs             RequestId extractor and RequestIdPlugin
  secure_headers.rs         SecureHeaders plugin + builder (CSP, HSTS, X-Frame-Options, ...)
  service.rs                ServiceComponent trait
  health.rs                 HealthIndicator trait, HealthBuilder, HealthState, /health endpoints
  sse.rs                    SseBroadcaster, SseStream for Server-Sent Events
  ws.rs                     WsStream, WsHandler, WsBroadcaster, WsRooms (feature = "ws")
  state.rs                  R2eState wrapper type
  type_list.rs              Heterogeneous type list (TNil, TCons, Contains, BuildableFrom) for compile-time DI
  types.rs                  Shared type definitions
  prelude.rs                Convenience re-exports
  validation.rs             Automatic validation via garde (autoref specialization)
  params.rs                 Params derive helpers (ParamError, parse_query_string)
  multipart.rs              Multipart extraction (feature = "multipart")

  config/
    mod.rs                  R2eConfig, ConfigValue, FromConfigValue, ConfigError — public API
    loader.rs               YAML file loader with profile support (application.yaml + application-{profile}.yaml)
    registry.rs             Config section registry (register_section, validate_section)
    secrets.rs              SecretResolver trait, DefaultSecretResolver (env var interpolation)
    typed.rs                Typed config value extraction
    validation.rs           Config key validation
    value.rs                ConfigValue enum (String, Int, Float, Bool, List, Map)

  http/
    mod.rs                  HTTP module re-exports (Router, StatusCode, HeaderMap, ...)
    body.rs                 Request/response body types
    extract.rs              Custom extractors (FromRef, FromRequestParts wrappers)
    header.rs               Header utilities (Parts, HttpRequest)
    middleware.rs            Middleware types (Next)
    response.rs             Response builders (IntoResponse)
    routing.rs              Routing utilities
    ws.rs                   WebSocket re-exports

tests/
  integration.rs            Full-stack integration tests (AppBuilder -> TestApp)
  beans.rs                  Bean/AsyncBean/Producer DI tests
  config.rs                 R2eConfig loading, get, get_or, env overlay tests
  secrets.rs                SecretResolver tests
  guards.rs                 Guard, RolesGuard, GuardContext tests
  interceptors.rs           Interceptor trait tests
  health.rs                 HealthIndicator, HealthBuilder, HealthState tests
  error.rs                  HttpError -> HTTP response tests
  plugin.rs                 DeferredAction, DeferredContext tests
  managed.rs                ManagedResource lifecycle tests
  request_id.rs             RequestId extraction tests
  secure_headers.rs         SecureHeaders builder and default tests
  ws.rs                     WsBroadcaster, WsRooms tests (feature = "ws")
  sse.rs                    SseBroadcaster tests
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
  identity.rs               AuthenticatedUser struct, IdentityBuilder trait, impl_claims_identity_extractor! macro
  jwt.rs                    JwtClaimsValidator, JwtValidator — token validation with static key or JWKS
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
  lib.rs                    Scheduler PreStatePlugin, SchedulerHandle, task runner loop
  types.rs                  ScheduleConfig, ScheduledTaskDef<T>, ScheduledTask trait, ScheduledResult

tests/
  scheduler.rs              Scheduler lifecycle tests
  types.rs                  ScheduleConfig parsing and task definition tests
  scheduler_test.rs         Additional scheduler tests
  plugin_test.rs            Scheduler plugin integration tests
```

---

## r2e-data — Data abstractions

Driver-independent database traits (no SQLx/Diesel dependency).

```
src/
  lib.rs                    Entry point
  entity.rs                 Entity trait (table_name, columns)
  repository.rs             Repository trait (find_by_id, find_all, create, update, delete)
  page.rs                   Page<T>, Pageable for pagination
  error.rs                  DataError enum
```

---

## r2e-data-sqlx — SQLx backend

```
src/
  lib.rs                    Entry point
  repository.rs             SqlxRepository implementation
  tx.rs                     Tx<'a, DB> transaction wrapper, HasPool trait
  migration.rs              Migration runner utilities
  error.rs                  SQLx -> DataError bridging
```

---

## r2e-data-diesel — Diesel backend (skeleton)

```
src/
  lib.rs                    Entry point
  repository.rs             DieselRepository stub
  error.rs                  Diesel -> DataError bridging
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

OpenAPI 3.0.3 spec generation from route metadata.

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
  middleware.rs              Tracing middleware
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
  user_controller_test.rs   Integration tests for user endpoints
  consumer_test.rs          Event consumer tests
```

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
