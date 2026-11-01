# Observability

R2E provides built-in observability through request tracing, request IDs, and metric interceptors. These tools work together to give visibility into your application's behavior.

Two concerns, two plugins — keep them apart:

| Concern | Owner | Config section |
|---|---|---|
| **Where logs go** — format, filter, ansi (the subscriber) | `build_state()`, automatically (layers added with `.tracing_layer(..)`) | `tracing:` |
| **What each request logs** — span, summary line, request id, exclusions | the `HttpTrace` plugin | `trace:` |
| Both, plus OTLP export and `traceparent` propagation | `Observability` (feature `observability`) | `observability:` + `trace:` |

## HttpTrace plugin

`HttpTrace` is the per-request HTTP layer: **one** span and **one** summary line
per request, request-id resolution, and path exclusions.

```rust
use r2e::prelude::*;

AppBuilder::new()
    .plugin(HttpTrace::new())
    .build_state()
    .await
    .serve("0.0.0.0:3000")
    .await;
```

### What it emits

A span named `request` at INFO on target `r2e::http`, **entered for the whole
handler future** — so every line your handler logs is decorated with the
request's `route` and `request_id`. Its fields are `method`, `route` (the
bounded route template, e.g. `/users/{id}` — never the raw path), `request_id`,
and the configured `capture-headers`. Inside it, one summary event:

```text
INFO  r2e::http: request completed status=200 latency_ms=3.2
ERROR r2e::http: request completed status=503 latency_ms=12.0
```

The level is INFO below 500 and ERROR at 5xx. `latency_ms` is measured to the
response **head**, so a streaming body is not counted (which is why the
Prometheus layer keeps its own timer).

The raw path and query string are deliberately **off**: they carry ids and
tokens, and the span decorates every handler log line. Opt in per app with
`record-path` / `record-query`, which put them on the summary **event** only —
never on the span.

### Request ids

Unless `request-id: false`, the layer reads an inbound `x-request-id`, mints a
UUID v4 when there is none, records it on the span, publishes it as the
`RequestId` request extension (so handlers can extract it), and echoes it on the
response. `RequestIdPlugin` does exactly this and nothing else; installing both,
in either order, is harmless — the id is resolved once and both agree.

### Configuration

```yaml
trace:
  enabled: true
  exclude-paths: ["/health", "/metrics"]   # prefix match, raw path OR route label
  request-id: true
  record-path: false                       # raw path on the summary event only
  record-query: false
  capture-headers: []                      # recorded on the span
  summary: true                            # the one-line "request completed"
  request-event: false                     # opt-in DEBUG "request started"
```

`enabled: false` removes the layer entirely — no span, no event, no request id.
An excluded path is passed through untouched.

The same knobs are available on the builder, which **wins over the file** — it
is the "I mean exactly this" lane:

```rust
AppBuilder::new()
    .plugin(
        HttpTrace::builder()
            .exclude_paths(["/health", "/metrics", "/internal"])
            .capture_header("user-agent")
            .record_path(true)
            .build(),
    )
```

Precedence per knob: **explicit builder setting > the app's `trace:` section >
`HttpTrace::preset(..)` > built-in default**. `preset` is the shared-baseline
lane: a company crate ships the house HTTP-log contract there and each service's
own `application.yaml` still has the last word without touching code.

### A custom span shape

`MakeRequestSpan` is the seam. Implement it to change the span's name and
fields; exclusions, request id, timing and the summary event stay with the
layer. That is exactly how `Observability` swaps in OpenTelemetry semantic
conventions without a second layer.

```rust,ignore
use r2e::{MakeRequestSpan, RequestOutcome, RequestHead};

struct MySpan;

impl MakeRequestSpan for MySpan {
    fn make_span(&self, req: &RequestHead<'_>, route: &str, request_id: Option<&str>) -> tracing::Span {
        tracing::info_span!(target: "r2e::http", "request", route, request_id)
    }
}

AppBuilder::new().plugin(HttpTrace::builder().make_span(MySpan).build())
```

### Enriching the span from a handler

The span maker is shared across requests, so it cannot carry per-request data
itself. The layer provides the channel instead, in two pieces.

**`RequestSpan`** — every traced request carries its span as a request
extension. A handler takes it as a parameter and records fields the span shape
declared `Empty`:

```rust,ignore
#[get("/current")]
async fn current(&self, span: RequestSpan) -> &'static str {
    span.record("session_id", "sess-42");
    "ok"
}
```

This works at any call depth (unlike `Span::current()`, which resolves to the
innermost entered span) and needs no task-local plumbing. On an excluded path
the extractor falls back to `Span::none()`, so `record` degrades to a no-op.

**`SpanState`** — when the summary *event* must carry values produced during
the request (span fields are write-only), implement
`MakeRequestSpan::make_state` to allocate a per-request slot. The layer
publishes it as a request extension (handlers write into it, through interior
mutability) and hands it back to `on_response`, which reads the values and
emits its own summary line:

```rust,ignore
use std::sync::Mutex;

#[derive(Default)]
struct Facts { session_id: Mutex<Option<String>> }

impl MakeRequestSpan for MySpan {
    fn make_span(&self, req: &RequestHead<'_>, route: &str, request_id: Option<&str>) -> tracing::Span {
        tracing::info_span!(target: "r2e::http", "request", route,
            session_id = tracing::field::Empty, status = tracing::field::Empty)
    }

    fn make_state(&self, _req: &RequestHead<'_>) -> Option<SpanState> {
        Some(SpanState::new(Facts::default()))
    }

    fn on_response(&self, span: &tracing::Span, outcome: &RequestOutcome<'_>, state: Option<&SpanState>) {
        let session = state
            .and_then(|s| s.get::<Facts>())
            .and_then(|f| f.session_id.lock().unwrap().clone());
        span.record("status", outcome.status.map(|s| s.as_u16()));
        let _enter = span.enter();
        tracing::info!(target: "r2e::http",
            status = outcome.status.map(|s| s.as_u16()),
            latency_ms = outcome.latency_ms(),
            session_id = session.as_deref(),
            "request completed");
    }
}
```

Field *names* remain compile-time (`tracing` derives them from static callsite
metadata) — that is why the enrichment point is your own `MakeRequestSpan`
rather than a generic `record(name, value)` on the layer.

## The log subscriber (`tracing:`)

R2E installs the global `tracing` subscriber **for you**, in exactly one place:
`build_state()`, as its first step. It reads your `tracing:` section (profile
and `R2E_*` overrides included; built-in defaults when there is none) and adds
every layer contributed with `.tracing_layer(..)` before it. There is no
plugin to install and nothing to call — `r2e::app_main!(MyApp)` and a
`tracing:` section are the whole setup.

```rust
use r2e::prelude::*;

AppBuilder::new()
    .load_config::<()>()          // the `tracing:` section comes from here
    .plugin(HttpTrace::new())     // the per-request span
    .build_state()                // installs the subscriber, first thing
    .await
    .serve("0.0.0.0:3000")
    .await;
```

> Upgrading from an earlier release? The old tracing plugins, the
> tracing init functions and the `tracing = …` macro argument were removed; each
> leftover use fails to build with a message naming the repository's
> `docs/migration/tracing-single-path.md`.

### Controlling log levels

Set `tracing.filter`, or the `RUST_LOG` environment variable, which wins when
set:

```bash
# Default (when neither is set)
RUST_LOG="info"

# Show all framework internals
RUST_LOG="debug"

# Production — only warnings and errors
RUST_LOG="warn"

# Fine-grained control
RUST_LOG="info,my_app=debug,r2e::http=debug"
```

An invalid directive never aborts the boot: the subscriber falls back to
`info` and logs one `ERROR` naming it.

### YAML configuration

```yaml
tracing:
  enabled: true        # false: R2E installs nothing (you own the subscriber)
  console: true        # false: no built-in console; contributed layers still run
  filter: "info,tower_http=debug,my_app=trace"
  format: json
  ansi: false
  target: true
  thread-ids: true
  thread-names: false
  file: true
  line-number: true
  level: true
  span-events: full
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enabled` | bool | `true` | `false`: R2E installs no subscriber and ignores contributed layers |
| `console` | bool | `true` | `false`: no built-in console layer (bring your own with `.tracing_layer(..)`) |
| `filter` | String | `"info"` | `EnvFilter` directive. `RUST_LOG` env var takes priority. |
| `format` | `pretty` / `json` | `pretty` | Log output format |
| `target` | bool | `true` | Print the module path in each log line |
| `thread-ids` | bool | `false` | Print thread IDs |
| `thread-names` | bool | `false` | Print thread names |
| `file` | bool | `false` | Print file name where the log originated |
| `line-number` | bool | `false` | Print line number |
| `level` | bool | `true` | Print the log level |
| `ansi` | bool | on when stdout is a terminal | Enable ANSI color codes |
| `span-events` | `none` / `new` / `close` / `active` / `full` | `close` | Which span lifecycle events to record |

Single-word keys can be overridden from the environment (`R2E_TRACING_ENABLED=false`,
`R2E_TRACING_FORMAT=json`, `R2E_TRACING_FILTER=debug`). An invalid `tracing:`
section does not abort the boot either: the defaults are installed and one
`ERROR` explains what was wrong.

### Flat JSON lines

`format: json` writes `tracing-subscriber`'s nested shape, with the message and
the event fields under `"fields"`. Log stores that index top-level keys
(VictoriaLogs, Loki) want one flat object per event instead — set
`tracing.json.flatten`:

```yaml
tracing:
  format: json
  json:
    flatten: true        # default false: the nested shape
    message-key: _msg    # default "message" (VictoriaLogs reads `_msg`)
    current-span: true   # default true: "span": {"name": .., <fields>}
    span-list: true      # default true: "spans": [root, .., leaf]
```

```json
{"_msg":"request completed","level":"INFO","target":"r2e::http","timestamp":"2026-10-31T09:12:03.123456Z","status":200,"span":{"name":"request","route":"/users"},"spans":[{"name":"request","route":"/users"}]}
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `json.flatten` | bool | `false` | One flat object per event |
| `json.message-key` | String | `"message"` | Key of the message; the event's target when it has none |
| `json.current-span` | bool | `true` | Add the event's span as `span` |
| `json.span-list` | bool | `true` | Add the span stack, root first, as `spans` |

Event fields keep their JSON types; `LogFields` entries (below) are strings.
The formatter's own keys — the message key, `timestamp` (RFC 3339, UTC),
`level`, `target`, `thread_id`, `thread_name`, `file`, `line`, `span`, `spans`
— win over a field of the same name. Events from the `log` crate keep their
real level and target, without the `log.*` bookkeeping fields.
`R2E_TRACING_JSON_FLATTEN=true` toggles flattening from the environment;
`message-key` (a kebab key) is set in YAML, or from a variable with
`message-key: "${LOG_MESSAGE_KEY:_msg}"`.

### Adding layers: `tracing_layer`

Anything beyond the knobs above is a `tracing_subscriber` layer contributed
**before** `build_state()` — on the builder, or from a plugin's `setup` with
`ctx.tracing_layer(..)`. They are installed together with the subscriber, once.

```rust
use r2e::prelude::*;
use r2e::Suppress;

AppBuilder::new()
    .load_config::<()>()
    // Drop one known-benign upstream event (exact level + target + message).
    .tracing_layer(Suppress::error("some_dep::conn", "connection reset during shutdown"))
    .build_state()
    .await;
```

- **`Suppress`** — `Suppress::error(target, message)` /
  `Suppress::new(level, target, message)` vetoes exactly one event shape for
  the console and every other layer (OpenTelemetry included).
- **`LogFields`** — a layer can attach fields to a span (`on_new_span` /
  `on_enter`); the console prints them on every line inside that span, the
  closest span winning on a key collision. This is how `Observability` adds
  `trace_id` / `span_id`.
- **Your own console** — `tracing.console: false` +
  `.tracing_layer(fmt::layer().compact())`. The global filter still applies.

### Boot logs

The subscriber only exists once `build_state()` has read `tracing:` and every
plugin has contributed — after `App::setup` and config loading already ran.
So `r2e::launch` / `app_main!` / `TestApp::boot` capture first: what is logged
before the install is buffered and replayed through the real subscriber, and
if the boot fails before the install, the buffered lines are printed to stderr
ahead of the error. What is buffered follows `RUST_LOG` (full directive
syntax, e.g. `RUST_LOG=info,my_app=debug`; unset or invalid: `INFO` and
above) — `tracing.filter` is not loaded yet then, but still applies to the
replay. Limits: replayed lines carry no parent span and are timestamped at
replay, not emission (a slow upstream wait in `App::setup` shows up stamped
at the install; put it after `build_state()` or in an `#[on_start]` hook to
log it live), and the buffer holds 1024 events (one `WARN` reports the
overflow). A hand-written `main` that calls `build_state()` without
`launch` should call `let _boot = r2e::capture_boot_logs();` first to get the
same behaviour.

### Owning the subscriber yourself

A subscriber is a one-shot process global: the first install wins. If you
build your own `tracing_subscriber` stack, set:

```yaml
tracing:
  enabled: false
```

R2E then installs nothing and ignores contributed layers — add the ones you
need (`Suppress`, `r2e_observability::tracing_setup::otel_layer()` +
`OtelIds`, …) to your own `Registry`. If you install a subscriber *without*
setting `enabled: false`, yours still wins, and `build_state()` logs one `WARN`
naming how many contributed layers it had to drop.

### In tests

`TestApp::boot` installs from the **test** profile (`application-test.yaml`),
like any other configuration; `tracing: { enabled: false }` there quiets a
whole test binary. `#[r2e::test]` itself installs nothing. A test with no app
that still wants log output installs once:

```rust
let _ = r2e::r2e_tracing::install(&r2e::TracingOptions::default(), vec![]);
```

## OpenTelemetry observability

Enable the `observability` feature and install `Observability` (instead of
`HttpTrace`) for distributed traces:

```rust
use r2e::r2e_observability::Observability;

builder
    .plugin(Observability::from_env("my-service"))
    .build_state()
    .await
```

`from_env` reads the standard OTLP endpoint, service-name, protocol, and
sampler variables. With no `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` or
`OTEL_EXPORTER_OTLP_ENDPOINT`, it exports nothing and only installs the HTTP
trace layer — the same plugin therefore works locally and deployed, without
conditional subscriber initialization in your code.

`Observability` does **not** build a subscriber. In its `setup` it
contributes two layers with `ctx.tracing_layer(..)` — the OpenTelemetry layer,
then `OtelIds`, which writes `trace_id`/`span_id` as `LogFields` so they appear
on console lines — and `build_state()` installs them with R2E's one subscriber.
A plain `r2e::app_main!(MyApp)` is all it needs; the console is configured by
`tracing:` exactly as without the plugin, and `Suppress` layers apply to the
exported spans too. Only `observability.tracing.enabled` (the OTLP switch) is
read under `observability.tracing`; any other key there is ignored and
reported by one `ERROR` at boot (see
the repository's `docs/migration/tracing-single-path.md`).

R2E exports OTLP/HTTP to `http://localhost:4318/v1/traces` by default. A
pathless HTTP(S) endpoint receives `/v1/traces` automatically; requesting gRPC
logs a warning and uses HTTP. Events emitted inside OTel spans include
`trace_id` and `span_id` in pretty and JSON logs.

To instrument outgoing reqwest calls, wrap the client once:

```rust
use r2e::r2e_observability::traced_reqwest_client;

let http = traced_reqwest_client(reqwest::Client::new());
```

Every request then runs in an OpenTelemetry **client** span (`otel.kind =
"client"`, named `HTTP {method}`) carrying the HTTP-client semantic
conventions — `http.request.method`, `server.address`, `server.port`,
`url.full` (credentials stripped), `http.response.status_code`, and
`otel.status_code` / `error.message` on failure — and the injected
`traceparent` carries that client span's id. Tracing backends build their
service graph by pairing a CLIENT span with its direct SERVER child (Tempo's
metrics-generator, Jaeger, Grafana), so this is what produces the
`caller → callee` edge and `traces_service_graph_request_client_seconds`.

Span names must stay low-cardinality, so the URL is never part of them. Opt
into route templates per request with
`.with_extension(OtelPathNames::known_paths(["/items/{id}"])?)` (names become
`GET /items/{id}`), or force a name with `.with_extension(OtelName("…".into()))`.
`DisableOtelPropagation` turns header injection off for one request. All three
are re-exported from `r2e_observability`.

Pass that `ClientWithMiddleware` to client SDKs. SDKs restricted to a plain
reqwest client can instead call `inject_current_context(headers)` in their
single request-construction chokepoint — but note that this only injects the
*current* span's context and opens no client span: the trace is still joined
across services, yet the service graph shows no edge between them. Wrap the
call in your own `otel.kind = "client"` span if you need one.

## RequestId plugin

The `RequestIdPlugin` assigns a unique identifier to every request, enabling correlation across log lines and distributed systems. `HttpTrace` already does this
(unless `trace.request-id: false`), so this plugin is for apps that want request
ids **without** the HTTP trace layer; installing both, in either order, is
harmless — the id is resolved once and both agree on it.

```rust
AppBuilder::new()
    .plugin(RequestIdPlugin)
    .build_state()
    .await
    .serve("0.0.0.0:3000")
    .await;
```

### Behavior

1. Reads `X-Request-Id` from the incoming request headers.
2. If absent, generates a UUID v4.
3. Stores the ID as a request extension (available to handlers).
4. Copies the ID into the response `X-Request-Id` header.

This means upstream proxies and API gateways can set the request ID, and R2E will propagate it. If no ID is provided, one is generated automatically.

### Extracting the request ID in handlers

`RequestId` implements `FromRequestParts`, so you can use it as a handler parameter:

```rust
use r2e::prelude::*;

#[controller(path = "/api")]
pub struct ApiController {
    #[inject] service: MyService,
}

#[routes]
impl ApiController {
    #[get("/")]
    async fn handle(&self, req_id: RequestId) -> String {
        tracing::info!(%req_id, "processing request");
        format!("request: {}", req_id)
    }
}
```

`RequestId` implements `Display`, so it works directly with tracing's `%` format and with string formatting.

## Metric interceptors

R2E provides two metric interceptors in `r2e-utils` for instrumenting individual handler methods. Both emit structured log events via `tracing`, making them compatible with any log aggregation system.

### `Counted` — Request counting

Logs a counter event each time a handler is invoked:

```rust
use r2e::prelude::*;

#[routes]
impl UserController {
    #[get("/")]
    #[intercept(Counted::new("user_list_total"))]
    async fn list(&self) -> Json<Vec<User>> {
        Json(self.service.list().await)
    }
}
```

Each invocation produces a log line like:

```
INFO user_list counted metric=user_list_total
```

You can change the log level:

```rust
#[intercept(Counted::new("user_list_total").with_level(LogLevel::Debug))]
```

### `MetricTimed` — Duration metrics

Records the execution duration of a handler as a named metric:

```rust
#[routes]
impl UserController {
    #[get("/")]
    #[intercept(MetricTimed::new("user_list_duration"))]
    async fn list(&self) -> Json<Vec<User>> {
        Json(self.service.list().await)
    }
}
```

Each invocation produces:

```
INFO user_list metric=user_list_duration elapsed_ms=42
```

Like `Counted`, you can adjust the log level:

```rust
#[intercept(MetricTimed::new("user_list_duration").with_level(LogLevel::Warn))]
```

### Difference from `Timed`

`Timed` is a general-purpose timing interceptor that logs execution time as a plain message (e.g., `elapsed_ms=42`). It also supports a threshold to suppress fast calls.

`MetricTimed` is designed for metric collection: it includes a named metric identifier in the log output, making it easy to filter and aggregate in log-based monitoring tools (Loki, CloudWatch, Datadog).

| Interceptor | Output format | Use case |
|---|---|---|
| `Timed::new()` | `elapsed_ms=42` | Development logging |
| `Timed::threshold(100)` | Only logs if >100ms | Slow query detection |
| `MetricTimed::new("name")` | `metric=name elapsed_ms=42` | Metric collection |

## Combining everything

A typical production setup uses all observability features together:

```rust
use r2e::prelude::*;

// Application setup — the subscriber comes from the `tracing:` section,
// installed by `build_state()`
AppBuilder::new()
    .load_config::<RootConfig>()
    .plugin(HttpTrace::new())    // Per-request span, summary line, request IDs
    .plugin(Health)              // Health check endpoint
    .build_state()
    .await
    .register_controller::<ApiController>()
    .serve("0.0.0.0:3000")
    .await;
```

```rust
#[controller(path = "/users")]
pub struct UserController {
    #[inject] service: UserService,
}

#[routes]
#[intercept(Logged::info())]
impl UserController {
    #[get("/")]
    #[intercept(Counted::new("users_list_total"))]
    #[intercept(MetricTimed::new("users_list_duration"))]
    async fn list(&self, req_id: RequestId) -> Json<Vec<User>> {
        tracing::info!(%req_id, "listing users");
        Json(self.service.list().await)
    }

    #[get("/:id")]
    #[intercept(MetricTimed::new("users_get_by_id_duration"))]
    async fn get_by_id(&self, Path(id): Path<i64>) -> Json<User> {
        Json(self.service.find(id).await)
    }
}
```

This produces structured log output with:

- Request IDs on every request/response (via `X-Request-Id` header)
- Entry/exit logging for all methods (via `Logged::info()`)
- Per-endpoint invocation counts (via `Counted`)
- Per-endpoint duration metrics (via `MetricTimed`)
- HTTP-level request spans and summary lines (via the `HttpTrace` plugin)

## Subscriber vs `HttpTrace` vs `Observability`

Three jobs — mix them freely:

| | Subscriber (`build_state()`) | `HttpTrace` | `Observability` |
|---|---|---|---|
| Crate | `r2e-core` (always available) | `r2e-core` (always available) | `r2e-observability` (feature `observability`) |
| Log subscriber | installs it (`tracing_subscriber` console + contributed layers) | — | contributes `tracing-opentelemetry` + `trace_id`/`span_id` fields to R2E's |
| Per-request span | — | `request` (short field names) | `request` (OTel semconv names) |
| Distributed tracing | No | No | Yes (OTLP export to Jaeger, Tempo, etc.) |
| Context propagation | No | No | Yes (W3C `traceparent`) |
| Configuration | `TracingConfig` — `tracing.*` | `HttpTraceConfig` — `trace.*` | `ObservabilityConfig` — `observability.*` (+ `trace.*` for the span) |

Install `HttpTrace` in every HTTP service: the subscriber is installed for
you, so it is usually the only one you name. Switch to `Observability` when
you need distributed tracing across microservices — it installs `HttpTrace`'s
layer itself with an OpenTelemetry span shape, so there is still exactly
**one** span per request; adding `HttpTrace` next to it would give you two. It
reads the same `trace:` section, so exclusions, request ids and `record-path`
are configured in one place either way.

Subscriber formatting options (format, ansi, thread IDs, etc.) live in one place, `TracingConfig` — the top-level `tracing.*` section — with or without `Observability`. `ObservabilityConfig` only carries the OTLP side (`observability.*`, where `observability.tracing.enabled` is the export switch).
