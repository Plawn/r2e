# Crate Map

R2E is organized as a workspace of focused crates. The `r2e` facade crate re-exports everything with feature gates.

## Crate overview

| Crate | Description |
|-------|-------------|
| `r2e` | Quarkus-like ergonomic layer over Axum — declarative controllers, compile-time DI, and zero runtime reflection |
| `r2e-cache` | TTL cache with pluggable backends for R2E - in-memory caching with expiration |
| `r2e-cli` | CLI tool for R2E - project scaffolding, code generation, and development server |
| `r2e-compile-tests` | (no description) |
| `r2e-core` | Core runtime for R2E web framework - AppBuilder, plugins, guards, and dependency injection |
| `r2e-data-diesel` | Diesel backend for R2E data layer (skeleton) |
| `r2e-data-sqlx` | SQLx backend for R2E data layer — SqlxRepository, Tx, HasPool, ManagedResource impl |
| `r2e-data` | Data access abstractions for R2E — Entity, Repository, Page, DataError (no driver deps) |
| `r2e-devtools` | Subsecond hot-reload integration for R2E |
| `r2e-events-iggy` | Apache Iggy event bus backend for R2E — persistent distributed event streaming |
| `r2e-events-kafka` | Apache Kafka event bus backend for R2E — distributed event streaming |
| `r2e-events-pulsar` | Apache Pulsar event bus backend for R2E — distributed event streaming |
| `r2e-events-rabbitmq` | RabbitMQ (AMQP 0-9-1) event bus backend for R2E — durable message queuing |
| `r2e-events` | In-process typed event bus for R2E - publish/subscribe with async handlers |
| `r2e-grpc` | gRPC server support for R2E framework |
| `r2e-http` | HTTP abstraction layer for R2E - sole owner of the axum dependency |
| `r2e-macros` | Procedural macros for R2E framework - Controller derive and routes attribute |
| `r2e-observability` | OpenTelemetry observability plugin for R2E — distributed tracing and context propagation |
| `r2e-oidc` | Embedded OIDC server plugin for R2E - issue JWT tokens without an external identity provider |
| `r2e-openapi` | OpenAPI 3.1 spec generation for R2E - automatic API documentation with Swagger UI |
| `r2e-openfga` | OpenFGA fine-grained authorization for R2E - Zanzibar-style relationship-based access control |
| `r2e-prometheus` | Prometheus metrics plugin for R2E - HTTP request tracking and /metrics endpoint |
| `r2e-rate-limit` | Token-bucket rate limiting for R2E - per-user, per-IP, or global rate limits |
| `r2e-scheduler` | Background task scheduler for R2E - interval, cron, and delayed task execution |
| `r2e-security` | JWT/OIDC security module for R2E - token validation, JWKS cache, and AuthenticatedUser extractor |
| `r2e-static` | Embedded static file serving with SPA support for R2E |
| `r2e-test` | Test utilities for R2E - TestApp HTTP client and TestJwt token generation |
| `r2e-utils` | Built-in interceptors for R2E - Logged, Timed, Cache, and CacheInvalidate |

## Dependency flow

```
r2e-http (HTTP abstraction - sole axum dependency)
    ^
r2e-macros (proc-macro, no runtime deps)
    ^
r2e-core (runtime foundation, re-exports r2e-http as `http` module)
    ^
r2e-security / r2e-events / r2e-scheduler / r2e-data / r2e-grpc
    ^
r2e-data-sqlx / r2e-cache / r2e-rate-limit / r2e-openapi / r2e-utils
r2e-prometheus / r2e-observability / r2e-oidc / r2e-openfga / r2e-static
r2e-events-iggy / r2e-events-kafka / r2e-events-pulsar / r2e-events-rabbitmq
r2e-devtools / r2e-test
    ^
r2e (facade)
    ^
your application
```

## Feature flags

The `r2e` facade crate gates sub-crates behind features.

**Default features:** `security`, `events`, `utils`

| Feature | Crates / effect |
|---------|----------------|
| `security` | r2e-security |
| `events` | r2e-events |
| `utils` | r2e-utils |
| `data` | r2e-data |
| `data-sqlx` | data, r2e-data-sqlx |
| `data-diesel` | data, r2e-data-diesel |
| `sqlite` | data-sqlx, r2e-data-sqlx/sqlite |
| `postgres` | data-sqlx, r2e-data-sqlx/postgres |
| `mysql` | data-sqlx, r2e-data-sqlx/mysql |
| `events-iggy` | events, r2e-events-iggy |
| `events-kafka` | events, r2e-events-kafka |
| `events-pulsar` | events, r2e-events-pulsar |
| `events-rabbitmq` | events, r2e-events-rabbitmq |
| `scheduler` | r2e-scheduler |
| `cache` | r2e-cache |
| `rate-limit` | r2e-rate-limit |
| `oidc` | r2e-oidc |
| `openapi` | r2e-openapi |
| `prometheus` | r2e-prometheus |
| `openfga` | r2e-openfga |
| `observability` | r2e-observability |
| `grpc` | r2e-grpc |
| `static` | r2e-static |
| `ws` | r2e-core/ws |
| `multipart` | r2e-core/multipart |
| `dev-reload` | r2e-devtools, r2e-core/dev-reload |
| `full` | All of the above (except `dev-reload`) |

## Using sub-crates directly

While most applications should use the `r2e` facade, you can depend on individual crates:

```toml
[dependencies]
r2e-core = "0.1"
r2e-macros = "0.1"
r2e-security = "0.1"
```

The proc macros use `proc-macro-crate` for dynamic path detection — they check for `r2e` first, then fall back to `r2e-core`. This means generated code uses `::r2e::` paths when using the facade, or `::r2e_core::` when using crates directly.
