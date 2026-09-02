---
topic: observability
features: observability
tokens: ~900
requires: plugins
---

## Observability (OpenTelemetry)

### TL;DR

- Enable feature `observability` and install
  `Observability::new(ObservabilityConfig::new("my-service"))`; it is a superset
  of `Tracing` — never install both.
- Alternatives to the code form: `Observability::from_config(&config, "my-service")`
  (YAML `observability.*`) or `Observability::from_env("my-service")` (`OTEL_*`).
- Export is OTLP/HTTP: default `http://localhost:4318/v1/traces`, a pathless
  endpoint gets `/v1/traces` appended, and requesting gRPC warns and uses HTTP.
- W3C `traceparent` propagation is automatic, and logs inside traced spans carry
  `trace_id` / `span_id`.
- Record a request header on the span with `.capture_header("x-tenant-id")`.
- Instrument outgoing calls once per client with
  `traced_reqwest_client(reqwest::Client::new())` — its client span's id is what
  the injected `traceparent` carries, which is what service-graph edges need.
- Keep client-span names low-cardinality with
  `.with_extension(OtelPathNames::known_paths([...]))` or `OtelName(...)`.
- `inject_current_context(request.headers_mut())` is the headers-only fallback:
  no client span, so no service-graph edge unless you open one yourself.
- `from_env` with no endpoint installs the normal R2E tracing subscriber and no
  OTLP exporter.

Requires feature: `observability`. **Superset of `Tracing`** — do NOT install both.

```rust
use r2e::r2e_observability::{Observability, ObservabilityConfig};

# fn __doc(b: AppBuilder) -> impl Sized {
b.plugin(Observability::new(
    ObservabilityConfig::new("my-service")
        .with_service_version("1.0.0")
        .with_endpoint("http://otel-collector:4318/v1/traces")
        .capture_header("x-tenant-id"),
))
// or from YAML (observability.* keys): .plugin(Observability::from_config(&config, "my-service"))
// or from standard OTEL_* env vars (tracing-only when no endpoint is set):
// .plugin(Observability::from_env("my-service"))
# }
```

OTLP/HTTP export (Jaeger, Tempo, …) + W3C `traceparent` propagation. The
default endpoint is `http://localhost:4318/v1/traces`; pathless HTTP(S)
endpoints receive `/v1/traces` automatically. Requesting gRPC logs a warning
and uses HTTP. Logs inside traced spans include `trace_id` and `span_id`.

Outgoing reqwest instrumentation is opt-in once per client:

```rust
use r2e::r2e_observability::{inject_current_context, traced_reqwest_client};

let http = traced_reqwest_client(reqwest::Client::new());
// Pass `http` to SDKs accepting reqwest_middleware::ClientWithMiddleware.
// Each request runs in an `otel.kind = "client"` span ("HTTP GET", semconv
// attrs: http.request.method, server.address/port, url.full,
// http.response.status_code, error.message) and the injected `traceparent`
// carries THAT span's id — required for service-graph edges (Tempo, Jaeger).
// Low-cardinality route names per request:
//   .with_extension(OtelPathNames::known_paths(["/items/{id}"])?)  // "GET /items/{id}"
//   .with_extension(OtelName("fetch-item".into()))
// For a plain reqwest chokepoint instead (headers only, NO client span, so no
// service-graph edge unless you open your own client span):
// inject_current_context(request.headers_mut());
```

`Observability::from_env` reads `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` (preferred),
`OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_SERVICE_NAME`, protocol variables,
`OTEL_TRACES_SAMPLER`, and `OTEL_TRACES_SAMPLER_ARG`. With no endpoint it
installs the normal R2E tracing subscriber/layer and no OTLP exporter.
