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

```yaml
observability:
  otlp-endpoint: "http://otel-collector:4318/v1/traces"
  otlp-protocol: http
  sampling-ratio: 1.0
  tracing:
    enabled: true        # OTLP export switch — the only `observability.tracing.*` key

# The console output is the app's one subscriber, configured here:
tracing:
  filter: "info,tower_http=debug"
  format: json
```

The plugin does **not** build a subscriber. It contributes to the one R2E
installs from the top-level `tracing:` section (see `TracingConfig`): the
OpenTelemetry layer goes into the subscriber's late-layer slot
(`r2e_core::contribute_layer`), and a field enricher (`r2e_core::add_enricher`)
adds `trace_id`/`span_id` to log lines. Any other `observability.tracing.*`
key is ignored and reported by one `ERROR` at boot — see
[`docs/migration/observability-tracing.md`](../docs/migration/observability-tracing.md).

### Programmatic configuration

```rust
use r2e::r2e_observability::{Observability, ObservabilityConfig};

let config = ObservabilityConfig::new("my-service")
    .with_service_version("1.0.0")
    .with_endpoint("http://otel-collector:4318/v1/traces")
    .with_sampling_ratio(0.25);
```

### Loading from R2eConfig

```rust
let obs = Observability::from_config(&r2e_config, "my-service");
```

This reads the `observability.*` keys.

### Own subscriber

An application that installs its own `tracing` subscriber (instead of letting
R2E install it) must host the late-layer slot, first, on its `Registry` —
otherwise nothing is exported and the plugin warns once:

```rust
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

tracing_subscriber::registry()
    .with(r2e::r2e_core::late_layers())   // first, on the Registry
    .with(tracing_subscriber::fmt::layer())
    .with(r2e::r2e_core::NoiseFilter)
    .init();
```

### Loading standard OpenTelemetry environment variables

```rust
.plugin(Observability::from_env("my-service"))
```

When no OTLP endpoint is configured, `from_env` only installs the HTTP trace
layer (and R2E's subscriber, if nothing installed it yet), without an exporter. When an endpoint is present, it enables OTLP export,
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
both pretty and JSON formats (a field enricher on R2E's console layer). Logs
outside an OTel span are unchanged.

## Breaking changes

Since 0.9.0 the plugin no longer builds a subscriber: `ObservabilityConfig::tracing`,
`with_tracing_config`, `with_log_format` and the `observability.tracing.*` subscriber
keys are gone — configure the console with the top-level `tracing:` section. See
[`docs/migration/observability-tracing.md`](../docs/migration/observability-tracing.md).

## License

Apache-2.0
