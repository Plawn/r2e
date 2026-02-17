# r2e-observability

OpenTelemetry observability plugin for R2E — distributed tracing and context propagation.

## Overview

Integrates [OpenTelemetry](https://opentelemetry.io/) for distributed tracing across services. Installs as a Tower layer that creates trace spans for each HTTP request and propagates trace context via W3C headers.

## Usage

Via the facade crate:

```toml
[dependencies]
r2e = { version = "0.1", features = ["observability"] }
```

## Setup

```rust
use r2e::r2e_observability::Observability;

AppBuilder::new()
    .build_state::<AppState, _>()
    .await
    .with(Observability::new("my-service"))
    .register_controller::<UserController>()
    .serve("0.0.0.0:3000")
    .await;
```

## Feature flags

| Feature | Default | Description |
|---------|---------|-------------|
| `otlp` | **yes** | OTLP exporter for sending traces to collectors (Jaeger, Grafana Tempo, etc.) |

## Capabilities

- **Distributed tracing** — creates spans for each HTTP request with method, path, status code
- **Context propagation** — propagates W3C `traceparent` / `tracestate` headers across services
- **OTLP export** — sends traces to any OpenTelemetry-compatible collector
- **Integration with `tracing`** — bridges Rust's `tracing` ecosystem with OpenTelemetry

## Configuration

The OTLP exporter is configured via standard OpenTelemetry environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` | Collector endpoint |
| `OTEL_SERVICE_NAME` | — | Service name for traces |

## License

Apache-2.0
