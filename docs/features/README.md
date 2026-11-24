# R2E — Feature Guide

## Overview

R2E provides 26 main features, each documented in a dedicated file.

| # | Feature | File | Crate |
|---|---------|------|-------|
| 1 | Configuration | [01-configuration.md](./01-configuration.md) | `r2e-core` |
| 2 | Validation | [02-validation.md](./02-validation.md) | `r2e-core` |
| 3 | Error Handling | [03-error-handling.md](./03-error-handling.md) | `r2e-core` |
| 4 | Interceptors | [04-interceptors.md](./04-interceptors.md) | `r2e-macros`, `r2e-utils` |
| 5 | OpenAPI | [05-openapi.md](./05-openapi.md) | `r2e-openapi` |
| 6 | Pagination / Managed Transactions | [06-data-repository.md](./06-data-repository.md) | `r2e-core`, SQLx/Diesel adapters |
| 7 | Events | [07-events.md](./07-events.md) | `r2e-events` |
| 8 | Scheduling | [08-scheduling.md](./08-scheduling.md) | `r2e-scheduler` |
| 9 | Development Mode | [09-dev-mode.md](./09-dev-mode.md) | `r2e-core` |
| 10 | Lifecycle Hooks | [10-lifecycle-hooks.md](./10-lifecycle-hooks.md) | `r2e-core` |
| 11 | JWT Security / Roles | [11-security-jwt.md](./11-security-jwt.md) | `r2e-security` |
| 12 | Testing | [12-testing.md](./12-testing.md) | `r2e-test` |
| 13 | Lifecycle, DI & Performance | [13-lifecycle-injection-performance.md](./13-lifecycle-injection-performance.md) | `r2e-core` / `r2e-macros` |
| 14 | WebSocket | [14-websocket.md](./14-websocket.md) | `r2e-http` |
| 15 | SSE | [15-sse.md](./15-sse.md) | `r2e-http` |
| 16 | Multipart | [16-multipart.md](./16-multipart.md) | `r2e-http` |
| 17 | gRPC | [17-grpc.md](./17-grpc.md) | `r2e-grpc` |
| 18 | QUIC / HTTP/3 | [18-quic.md](./18-quic.md) | `r2e-http` |
| 19 | Sharded Serving (SO_REUSEPORT) | [19-sharded-serving.md](./19-sharded-serving.md) | `r2e-core` |
| 20 | Proxy & Catch-All Routes | [20-proxy-catch-all.md](./20-proxy-catch-all.md) | `r2e-macros` / `r2e-core` |
| 21 | Dynamic Scheduled Tasks | [21-dynamic-scheduled-tasks.md](./21-dynamic-scheduled-tasks.md) | `r2e-scheduler` |
| 22 | Serve Lifecycle (Stop & Drain) | [22-serve-lifecycle.md](./22-serve-lifecycle.md) | `r2e-core` / `r2e-grpc` |
| 23 | OpenFGA Authorization (ReBAC, schema-first) | [23-openfga.md](./23-openfga.md) | `r2e-openfga` |
| 24 | Multi-Tenancy (per-tenant beans) | [24-tenancy.md](./24-tenancy.md) | `r2e-tenant` |
| 25 | MCP Server | [25-mcp.md](./25-mcp.md) | `r2e-mcp` |
| 26 | HTTP Stack (Layer Bands, CORS, Request Tracing) | [26-http-stack.md](./26-http-stack.md) | `r2e-core` |

## Crate Architecture

```
r2e               Facade crate; re-exports the subcrates behind feature flags. Apps depend on this.
r2e-rt            Async-runtime facade; the only crate naming tokio (re-exported as r2e::rt).
r2e-tracing       Process-global tracing subscriber: TracingOptions, boot-log buffer, Suppress, LogFields.
r2e-macros        Proc macros: #[controller] + #[routes] generate the HTTP handlers.
r2e-http          HTTP abstraction (sole owner of axum): IntoHttpResponse, JSON codec, QUIC/HTTP3.
r2e-core          Runtime: AppBuilder (HList state), Controller, guards/interceptors, R2eConfig,
                  HttpError, lifecycle, plugins, LayerSlot bands, Cors/HttpTrace, Pageable/Page.
r2e-security      JWT/JWKS validation, AuthenticatedUser, RoleExtractor.
r2e-events        In-process EventBus (typed pub/sub + request-reply); iggy/kafka/pulsar/rabbitmq backends.
r2e-scheduler     Interval/cron scheduling on the Executor pool.
r2e-executor      Managed task pool, #[async_exec], #[derive(BackgroundService)].
r2e-data-{sqlx,diesel}  Managed transactions + DataSource plugins (datasource.* config).
r2e-grpc          Tonic gRPC on its own port or multiplexed with HTTP; grpc-web.
r2e-mcp           MCP server: #[tool]/#[resource]/#[prompt] in #[routes]; OAuth resource-server auth.
r2e-cache         TtlCache + pluggable CacheStore bean.
r2e-rate-limit    Token-bucket RateLimiter, pluggable backend, RateLimitRegistry.
r2e-openapi       OpenAPI 3.1.0 generation, Swagger UI at /docs.
r2e-prometheus    HTTP metrics (full / layer_only / MetricsFacade).
r2e-observability OpenTelemetry tracing + context propagation via OTLP.
r2e-oidc          Embedded OAuth/JWT issuer (RS256, PKCE code flow, client_credentials).
r2e-openfga       OpenFGA authorization: .fga parser, model! macro, typed FgaCheck guards.
r2e-utils         Built-in interceptors: Logged, Timed, Cache, CacheInvalidate.
r2e-tenant        Multi-tenant bean routing: TenantResolver, Tenanted<T>, Tenancy/PerTenant plugins.
r2e-test          TestApp, TestJwt, TestSession, TestServer, WsTestClient, SSE helpers.
r2e-devservices   Testcontainers dev services: DevPostgres, DevRedis, DevKeycloak.
r2e-devtools      Subsecond hot-reload (feature dev-reload).
r2e-static        Embedded static files / SPA (rust_embed).
r2e-cli           r2e new / add / dev / generate / doctor / routes / docs.
```

## Topics covered elsewhere

These have no dedicated guide here; read the book chapter instead:

| Topic | Where |
|---|---|
| Observability (OpenTelemetry / OTLP) | [book: advanced/observability](../book/src/advanced/observability.md) |
| Request tracing (`HttpTrace`), CORS, layer bands | [26-http-stack.md](./26-http-stack.md) |
| Secure headers | [book: advanced/secure-headers](../book/src/advanced/secure-headers.md) |
| Prometheus metrics (`r2e-prometheus`) | the crate docs (`Prometheus`, `Prometheus::layer_only()`, `MetricsFacade`) and [book: core-concepts/plugins](../book/src/core-concepts/plugins.md) |
| Embedded OIDC issuer (`r2e-oidc`) | [book: security/embedded-oidc](../book/src/security/embedded-oidc.md) |
| Static files / SPA (`r2e-static`) | [book: advanced/static-files](../book/src/advanced/static-files.md) |
| Dev services (testcontainers) | [12-testing.md § Dev Services](./12-testing.md#dev-services-real-infrastructure) and [book: testing/test-setup](../book/src/testing/test-setup.md) |

## Migration guides

Step-by-step guides for breaking changes, in [`docs/migration/`](../migration/):

| Guide | Change |
|---|---|
| [di-provision-index.md](../migration/di-provision-index.md) | Generic DI builder bounds (r2e-core 0.12) |
| [layer-slots.md](../migration/layer-slots.md) | Layer bands — `LayerSlot` (r2e-core 0.10) |
| [tracing-single-path.md](../migration/tracing-single-path.md) | One tracing path through the builder (r2e-core 0.9) |
| [observability-tracing.md](../migration/observability-tracing.md) | `Observability` contributes to R2E's subscriber |
| [routes-newtype.md](../migration/routes-newtype.md) | `Routes` replaces `axum::Router` (r2e-core 0.8) |
| [rejection-layout.md](../migration/rejection-layout.md) | `Rejection` layout (r2e-core 0.7) |
| [response-schema.md](../migration/response-schema.md) | Body schemas through traits (0.6) |
| [error-projection.md](../migration/error-projection.md) | Error projection layers (0.5) |
| [plugin-api.md](../migration/plugin-api.md) | The plugin API (one `Plugin` trait) |
| [grpc-identity.md](../migration/grpc-identity.md) | gRPC identity on the HTTP extractor contract (r2e-grpc 0.10) |
| [mcp-identity.md](../migration/mcp-identity.md) | MCP mount stage and identity contract (r2e-mcp 0.10) |
| [mcp-registry.md](../migration/mcp-registry.md) | MCP global registry removed (r2e-mcp 0.7) |

## Quick Start

```bash
# Run the demo application
cargo run -p example-app

# In another terminal
curl http://localhost:3000/health           # → "OK"
curl http://localhost:3000/openapi.json     # → OpenAPI spec
curl http://localhost:3000/docs             # → API documentation interface
```
