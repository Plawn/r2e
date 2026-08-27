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
use r2e::r2e_observability::{Observability, ObservabilityConfig};

AppBuilder::new()
    .plugin(Observability::new(
        ObservabilityConfig::new("my-service")
            .with_service_version("1.0.0")
            .with_endpoint("http://otel-collector:4318/v1/traces"),
    ))
    .build_state()
    .await
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

### YAML configuration

The `ObservabilityConfig` embeds a `TracingConfig` for subscriber formatting. All subscriber options are under `observability.tracing.*`:

```yaml
observability:
  otlp-endpoint: "http://otel-collector:4318/v1/traces"
  otlp-protocol: http
  sampling-ratio: 1.0
  tracing:
    enabled: true
    filter: "info,tower_http=debug"
    format: json
    ansi: false
    target: true
    thread-ids: true
    file: false
    line-number: false
    span-events: close
```

### Programmatic configuration

```rust
use r2e::r2e_observability::{Observability, ObservabilityConfig};
use r2e::prelude::*;

let config = ObservabilityConfig::new("my-service")
    .with_tracing_config(
        TracingConfig::default()
            .with_format(LogFormat::Json)
            .with_ansi(false)
            .with_thread_ids(true),
    )
    .with_endpoint("http://otel-collector:4318/v1/traces");
```

The convenience method `.with_log_format(LogFormat::Json)` is also available and delegates to the embedded `TracingConfig`.

### Loading from R2eConfig

```rust
let obs = Observability::from_config(&r2e_config, "my-service");
```

This reads all `observability.*` keys including `observability.tracing.*`.

### Loading standard OpenTelemetry environment variables

```rust
.plugin(Observability::from_env("my-service"))
```

When no OTLP endpoint is configured, `from_env` installs ordinary R2E tracing
without an exporter. When an endpoint is present, it enables OTLP export,
honors `OTEL_SERVICE_NAME`, the OTLP protocol variables, and ratio sampler
settings. `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` takes precedence over the
general endpoint variable.

### Outgoing HTTP propagation

Wrap a reqwest client once and pass it to SDKs that accept
`reqwest_middleware::ClientWithMiddleware`:

```rust
use r2e::r2e_observability::traced_reqwest_client;

let http = traced_reqwest_client(reqwest::Client::new());
```

Every request then receives the current W3C `traceparent`/`tracestate`
automatically. For SDKs that own request construction, call
`inject_current_context(request.headers_mut())` at their request chokepoint.

### TracingConfig fields

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `filter` | String | `"info"` | `EnvFilter` directive. `RUST_LOG` env var takes priority. |
| `format` | `pretty` / `json` | `pretty` | Log output format |
| `target` | bool | `true` | Print the module path in each log line |
| `thread-ids` | bool | `false` | Print thread IDs |
| `thread-names` | bool | `false` | Print thread names |
| `file` | bool | `false` | Print file name where the log originated |
| `line-number` | bool | `false` | Print line number |
| `level` | bool | `true` | Print the log level |
| `ansi` | bool | `true` | Enable ANSI color codes |
| `span-events` | `none` / `new` / `close` / `active` / `full` | `close` | Which span lifecycle events to record |

### Environment variables

The OTLP exporter also respects standard OpenTelemetry environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` | — | Traces endpoint (takes precedence) |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | — | General collector endpoint; absence selects tracing-only mode |
| `OTEL_EXPORTER_OTLP_TRACES_PROTOCOL` | `http/protobuf` | Traces transport (`grpc` warns and falls back to HTTP) |
| `OTEL_SERVICE_NAME` | — | Service name for traces |
| `OTEL_TRACES_SAMPLER` | `parentbased_always_on` | Standard always-on/off or ratio sampler name |
| `OTEL_TRACES_SAMPLER_ARG` | `1.0` | Ratio for `traceidratio` samplers |

Pathless HTTP(S) endpoints automatically receive `/v1/traces`. The exporter
uses OTLP/HTTP; requesting gRPC emits an explicit warning rather than silently
sending HTTP to the usual gRPC port.

Logs emitted within an OpenTelemetry span include `trace_id` and `span_id` in
both pretty and JSON formats. Logs outside an OTel span are unchanged.

## Breaking changes

`ObservabilityConfig::log_format` field has been replaced by `ObservabilityConfig::tracing` (a `TracingConfig` struct). Use `.with_log_format()` or `.with_tracing_config()` instead. `LogFormat` is now re-exported from `r2e_core`.

## License

Apache-2.0
