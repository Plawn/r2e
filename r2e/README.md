# R2E

Quarkus-like ergonomic layer over [Axum](https://github.com/tokio-rs/axum) — declarative controllers, compile-time dependency injection, and zero runtime reflection.

## Overview

R2E is the main facade crate that re-exports all sub-crates through feature flags. Add a single dependency and enable only the features you need:

```toml
[dependencies]
r2e = { version = "0.1", features = ["sqlite", "scheduler", "openapi"] }
```

```rust
use r2e::prelude::*;
```

## Quick start

```rust
use r2e::prelude::*;

#[controller(path = "/hello")]
pub struct HelloController;

#[routes]
impl HelloController {
    #[get("/")]
    async fn hello(&self) -> &'static str {
        "Hello, R2E!"
    }
}

#[tokio::main]
async fn main() {
    AppBuilder::new()
        .build_state()
        .await
        .register_controller::<HelloController>()
        .serve("0.0.0.0:3000")
        .await
        .unwrap();
}
```

For real apps, prefer the `App` trait (`impl App { setup / build }`) launched
with `r2e::app_main!(MyApp)` — a single assembly path shared by production,
dev-reload, and tests (`TestApp::boot::<MyApp>()` / `#[r2e::test(app = MyApp)]`).

## Feature flags

| Feature         | Default | Crate                                |
|-----------------|---------|--------------------------------------|
| `security`      | **yes** | `r2e-security` — JWT/OIDC auth       |
| `events`        | **yes** | `r2e-events` — typed event bus       |
| `utils`         | **yes** | `r2e-utils` — Logged, Timed, Cache   |
| `data`          | no      | Compatibility marker; pagination is in core |
| `data-sqlx`     | no      | Managed SQLx transactions            |
| `data-diesel`   | no      | Managed Diesel transactions          |
| `sqlx-{sqlite,postgres,mysql}` | no | SQLx transaction + driver   |
| `diesel-{sqlite,postgres,mysql}` | no | Diesel transaction + driver |
| `scheduler`     | no      | `r2e-scheduler` — cron/interval (enables `executor`) |
| `executor`      | no      | `r2e-executor` — managed task pool   |
| `cache`         | no      | `r2e-cache` — TTL caching            |
| `rate-limit`    | no      | `r2e-rate-limit` — token-bucket      |
| `openapi`       | no      | `r2e-openapi` — OpenAPI 3.1.0 + docs UI |
| `prometheus`    | no      | `r2e-prometheus` — metrics endpoint  |
| `openfga`       | no      | `r2e-openfga` — Zanzibar authz       |
| `observability` | no      | `r2e-observability` — OpenTelemetry  |
| `oidc`          | no      | `r2e-oidc` — embedded OIDC server    |
| `grpc` / `grpc-reflection` | no | `r2e-grpc` — Tonic gRPC server |
| `events-{iggy,kafka,pulsar,rabbitmq}` | no | distributed EventBus backends |
| `static`        | no      | `r2e-static` — embedded static files + SPA |
| `ws`            | no      | WebSocket support                    |
| `multipart`     | no      | File upload support                  |
| `quic`          | no      | HTTP/3 + raw QUIC (not in `full` — heavy crypto deps) |
| `dev-reload`    | no      | `r2e-devtools` — Subsecond hot-reload (never in production) |
| `full`          | no      | All bundled framework modules (excludes `quic` and `dev-reload`) |

> Validation (via the `garde` crate) is always available and needs no feature flag.

## Workspace crates

| Crate | Description |
|-------|-------------|
| [`r2e-core`](../r2e-core) | Runtime foundation — AppBuilder, plugins, guards, DI, config |
| [`r2e-macros`](../r2e-macros) | Proc macros — `#[controller]`, `#[routes]`, `#[bean]` |
| [`r2e-security`](../r2e-security) | JWT validation, JWKS cache, `AuthenticatedUser` extractor |
| [`r2e-events`](../r2e-events) | In-process typed pub/sub event bus |
| [`r2e-scheduler`](../r2e-scheduler) | Background task scheduling (interval, cron) |
| [`r2e-executor`](../r2e-executor) | Managed task pool + background services |
| [`r2e-data-sqlx`](../r2e-data/backends/sqlx) | Managed SQLx transactions |
| [`r2e-data-diesel`](../r2e-data/backends/diesel) | Managed Diesel transactions |
| [`r2e-cache`](../r2e-cache) | TTL cache with pluggable backends |
| [`r2e-rate-limit`](../r2e-rate-limit) | Token-bucket rate limiting |
| [`r2e-openapi`](../r2e-openapi) | OpenAPI 3.1.0 spec generation + docs UI |
| [`r2e-prometheus`](../r2e-prometheus) | Prometheus metrics endpoint |
| [`r2e-observability`](../r2e-observability) | OpenTelemetry distributed tracing |
| [`r2e-oidc`](../r2e-oidc) | Embedded OIDC server (JWT issuance) |
| [`r2e-grpc`](../r2e-grpc) | Tonic-based gRPC server, multiplexed with HTTP |
| [`r2e-openfga`](../r2e-openfga) | OpenFGA fine-grained authorization |
| [`r2e-utils`](../r2e-utils) | Built-in interceptors (Logged, Timed, Cache) |
| [`r2e-static`](../r2e-static) | Embedded static file serving with SPA support |
| [`r2e-test`](../r2e-test) | Test helpers — TestApp, TestJwt |
| [`r2e-cli`](../r2e-cli) | CLI tool — scaffolding, codegen, dev server |

## License

Apache-2.0
