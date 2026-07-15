# R2E — Feature Guide

## Overview

R2E provides 20 main features, each documented in a dedicated file.

| # | Feature | File | Crate |
|---|---------|------|-------|
| 1 | Configuration | [01-configuration.md](./01-configuration.md) | `r2e-core` |
| 2 | Validation | [02-validation.md](./02-validation.md) | `r2e-core` |
| 3 | Error Handling | [03-error-handling.md](./03-error-handling.md) | `r2e-core` |
| 4 | Interceptors | [04-intercepteurs.md](./04-intercepteurs.md) | `r2e-macros` |
| 5 | OpenAPI | [05-openapi.md](./05-openapi.md) | `r2e-openapi` |
| 6 | Pagination / Managed Transactions | [06-data-repository.md](./06-data-repository.md) | `r2e-core`, SQLx/Diesel adapters |
| 7 | Events | [07-evenements.md](./07-evenements.md) | `r2e-events` |
| 8 | Scheduling | [08-scheduling.md](./08-scheduling.md) | `r2e-scheduler` |
| 9 | Development Mode | [09-dev-mode.md](./09-dev-mode.md) | `r2e-core` |
| 10 | Lifecycle Hooks | [10-lifecycle-hooks.md](./10-lifecycle-hooks.md) | `r2e-core` |
| 11 | JWT Security / Roles | [11-securite-jwt.md](./11-securite-jwt.md) | `r2e-security` |
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

## Crate Architecture

```
r2e-macros       Proc-macro. #[controller] + #[routes] generate Axum code.
r2e-core         Runtime. AppBuilder, Controller, HttpError, config, validation, cache, rate limiter.
r2e-security     JWT/JWKS, AuthenticatedUser, #[roles].
r2e-core         Pageable, Page, and the managed-resource lifecycle.
r2e-events       EventBus trait + LocalEventBus (typed pub/sub).
r2e-scheduler    Scheduled tasks (interval, cron) with CancellationToken.
r2e-openapi      OpenAPI 3.1.0 spec generation + Swagger UI.
r2e-static       Embedded static files with SPA support (wraps rust_embed).
r2e-test         TestApp (in-process HTTP client) + TestJwt.
r2e-cli          CLI (r2e new, r2e dev).
```

## Quick Start

```bash
# Run the demo application
cargo run -p example-app

# In another terminal
curl http://localhost:3000/health           # → "OK"
curl http://localhost:3000/openapi.json     # → OpenAPI spec
curl http://localhost:3000/docs             # → API documentation interface
```
