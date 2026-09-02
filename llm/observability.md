---
topic: observability
features: core, observability
tokens: ~1700
requires: plugins
---

## Request tracing (HttpTrace) and OpenTelemetry

### TL;DR

- **Per-request logging is the `HttpTrace` plugin** (core, no feature):
  `.plugin(HttpTrace::new())` → one span `request` (target `r2e::http`) + one
  `request completed` line (INFO, ERROR at 5xx) per request, with request ids.
- `Tracing` / `ConfiguredTracing` install the **subscriber only** (`tracing:`
  section) and add no HTTP layer. `r2e::launch` / `#[r2e::main]` /
  `#[r2e::test]` already install a subscriber, so most apps name only
  `HttpTrace`.
- The span carries the **bounded route template** (`/users/{id}`), never the raw
  path. `trace.record-path` / `record-query` put the raw values on the summary
  **event** only.
- Configure with the `trace:` YAML section or `HttpTrace::builder()`; a builder
  knob beats the file, and `HttpTrace::preset(cfg)` is the shared-baseline lane
  that the file beats. `trace.enabled: false` removes the layer entirely.
- `HttpTrace` resolves/echoes `x-request-id` itself; `RequestIdPlugin` is
  redundant with it but harmless in either order.
- Custom span shape: implement `MakeRequestSpan` and pass it to
  `HttpTrace::builder().make_span(..)` — exclusions, request id, timing and the
  summary stay with the layer.
- Enable feature `observability` and install
  `Observability::new(ObservabilityConfig::new("my-service"))`; it is a superset
  of `Tracing` **and** `HttpTrace` (it installs the same layer with an OTel span
  shape) — never install those alongside it.
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

### HttpTrace (core, no feature)

```rust
# fn __doc(b: AppBuilder) -> impl Sized {
b.plugin(HttpTrace::new())                       // defaults, still overridable from `trace:`
 .plugin(
     HttpTrace::builder()
         .exclude_paths(["/health", "/metrics", "/internal"])  // prefix: raw path OR route label
         .capture_header("user-agent")                          // recorded on the span
         .record_path(true)                                     // raw path on the EVENT only
         .build(),
 )
# }
```

```yaml
trace:
  enabled: true
  exclude-paths: ["/health", "/metrics"]   # default; [] traces everything
  request-id: true                         # read x-request-id or mint a UUID v4, echo it back
  record-path: false                       # raw path on the summary event (never the span)
  record-query: false
  capture-headers: []                      # invalid header name = boot error
  summary: true                            # the one-line "request completed"
  request-event: false                     # opt-in DEBUG "request started"
```

Emitted per request:

```text
INFO  r2e::http: request completed status=200 latency_ms=3.2
ERROR r2e::http: request completed status=503 latency_ms=12.0
```

The span is **entered for the whole handler future**, so every line the handler
logs inherits `route` and `request_id`. `latency_ms` stops at the response head
(a streamed body is not counted — that is why the Prometheus layer keeps its own
timer). An excluded path gets no span, no event and no request id.

### OpenTelemetry

Requires feature: `observability`. **Superset of `Tracing` and `HttpTrace`** —
do NOT install those alongside it. `Observability` installs the very same
`HttpTraceLayer` with an OpenTelemetry span shape (semconv
`http.request.method` / `http.route` / `http.response.status_code`, `otel.kind =
"server"`, parent context from the inbound `traceparent`), so there is exactly
one span per request, and it reads the same `trace:` section for exclusions,
request ids and `record-path`.

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
