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

#[derive(Controller)]
#[controller(path = "/hello", state = ())]
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
        .build_state::<(), _>()
        .await
        .register_controller::<HelloController>()
        .serve("0.0.0.0:3000")
        .await;
}
```

## Feature flags

| Feature         | Default | Crate                                |
|-----------------|---------|--------------------------------------|
| `security`      | **yes** | `r2e-security` — JWT/OIDC auth       |
| `events`        | **yes** | `r2e-events` — typed event bus       |
| `utils`         | **yes** | `r2e-utils` — Logged, Timed, Cache   |
| `data`          | no      | `r2e-data` — Entity, Repository      |
| `data-sqlx`     | no      | `r2e-data-sqlx` — SQLx backend       |
| `data-diesel`   | no      | `r2e-data-diesel` — Diesel backend   |
| `sqlite`        | no      | SQLx + SQLite driver                 |
| `postgres`      | no      | SQLx + PostgreSQL driver             |
| `mysql`         | no      | SQLx + MySQL driver                  |
| `scheduler`     | no      | `r2e-scheduler` — cron/interval      |
| `cache`         | no      | `r2e-cache` — TTL caching            |
| `rate-limit`    | no      | `r2e-rate-limit` — token-bucket      |
| `openapi`       | no      | `r2e-openapi` — OpenAPI 3.0 + UI    |
| `prometheus`    | no      | `r2e-prometheus` — metrics endpoint  |
| `openfga`       | no      | `r2e-openfga` — Zanzibar authz       |
| `observability` | no      | `r2e-observability` — OpenTelemetry  |
| `validation`    | no      | Input validation via `validator`     |
| `ws`            | no      | WebSocket support                    |
| `multipart`     | no      | File upload support                  |
| `full`          | no      | All of the above                     |

## Workspace crates

| Crate | Description |
|-------|-------------|
| [`r2e-core`](../r2e-core) | Runtime foundation — AppBuilder, plugins, guards, DI, config |
| [`r2e-macros`](../r2e-macros) | Proc macros — `#[derive(Controller)]`, `#[routes]`, `#[bean]` |
| [`r2e-security`](../r2e-security) | JWT validation, JWKS cache, `AuthenticatedUser` extractor |
| [`r2e-events`](../r2e-events) | In-process typed pub/sub event bus |
| [`r2e-scheduler`](../r2e-scheduler) | Background task scheduling (interval, cron) |
| [`r2e-data`](../r2e-data) | Data access abstractions (driver-independent) |
| [`r2e-data-sqlx`](../r2e-data-sqlx) | SQLx backend — repository, transactions, migrations |
| [`r2e-data-diesel`](../r2e-data-diesel) | Diesel backend (skeleton) |
| [`r2e-cache`](../r2e-cache) | TTL cache with pluggable backends |
| [`r2e-rate-limit`](../r2e-rate-limit) | Token-bucket rate limiting |
| [`r2e-openapi`](../r2e-openapi) | OpenAPI 3.0 spec generation + Swagger UI |
| [`r2e-prometheus`](../r2e-prometheus) | Prometheus metrics endpoint |
| [`r2e-observability`](../r2e-observability) | OpenTelemetry distributed tracing |
| [`r2e-openfga`](../r2e-openfga) | OpenFGA fine-grained authorization |
| [`r2e-utils`](../r2e-utils) | Built-in interceptors (Logged, Timed, Cache) |
| [`r2e-test`](../r2e-test) | Test helpers — TestApp, TestJwt |
| [`r2e-cli`](../r2e-cli) | CLI tool — scaffolding, codegen, dev server |

## License

Apache-2.0
