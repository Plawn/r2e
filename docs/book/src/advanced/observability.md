# Observability

R2E provides built-in observability through request tracing, request IDs, and metric interceptors. These tools work together to give visibility into your application's behavior.

## Tracing plugin

The `Tracing` plugin initializes structured logging and adds HTTP-level trace spans to every request.

```rust
use r2e::prelude::*;

AppBuilder::new()
    .plugin(Tracing)
    .build_state()
    .await
    .serve("0.0.0.0:3000")
    .await;
```

### What it does

1. Initializes the global `tracing` subscriber using `tracing_subscriber::fmt`.
2. Adds a `tower_http::TraceLayer` that logs requests and responses at the `DEBUG` level.

### Controlling log levels

Set the `RUST_LOG` environment variable:

```bash
# Default (when RUST_LOG is not set)
RUST_LOG="info"

# Show all framework internals
RUST_LOG="debug"

# Production — only warnings and errors
RUST_LOG="warn"

# Fine-grained control
RUST_LOG="info,my_app=debug,tower_http=trace"
```

The `init_tracing()` function is idempotent. If you need logs before the plugin is installed (e.g., during state construction), call it manually:

```rust
r2e::init_tracing();
```

## Configurable tracing

For full control over the log subscriber format, use `TracingConfig` and the `ConfiguredTracing` plugin. This lets you configure log format, ANSI colors, thread IDs, file names, and more — either programmatically or via YAML.

### YAML configuration

```yaml
tracing:
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

### Programmatic usage

```rust
use r2e::prelude::*;

// Build config programmatically
let tracing_config = TracingConfig::default()
    .with_format(LogFormat::Json)
    .with_ansi(false)
    .with_thread_ids(true)
    .with_filter("debug,hyper=warn");

AppBuilder::new()
    .plugin(Tracing::configured(tracing_config))
    .build_state()
    .await
    .serve("0.0.0.0:3000")
    .await;
```

### Loading from R2eConfig

When using `load_config`, you can read `TracingConfig` directly from your YAML:

```rust
let builder = AppBuilder::new().load_config::<RootConfig>();

// The builder exposes the loaded config via `r2e_config()`, so a
// config-driven plugin can be constructed before `build_state()`.
let tracing = Tracing::from_config(builder.r2e_config().unwrap());

builder
    .plugin(tracing)
    .build_state()
    .await
    .serve("0.0.0.0:3000")
    .await;
```

`Tracing::from_config()` reads the `tracing.*` keys from `R2eConfig`.

### Tracing under the canonical entrypoint

The canonical entrypoint — `r2e::app_main!(MyApp)` / `r2e::launch!` — installs
the subscriber **for you**, right after `App::setup` returns and before
`App::build` runs. It reads your own `tracing:` section to do it, so the YAML
above applies with no plugin and no code:

```yaml
tracing:
  format: json
  filter: "info,my_app=debug"
```

A missing `application.yaml` is silent (built-in defaults); an unreadable file
or an invalid `tracing:` section falls back to the defaults and warns through
the subscriber it just installed, so a bad config never costs you the log line
explaining the boot error that follows.

**A subscriber is a one-shot process global: the first install wins.** Since
the entrypoint installs before `App::build`, a `Tracing` / `ConfiguredTracing`
/ `Observability` plugin declared in `build` *loses that race*. Losing is
harmless when the plugin reads the same `tracing:` section — it is the same
subscriber either way, and R2E stays quiet. It is not harmless when the plugin
would have logged differently, or adds a layer of its own (OTLP!): R2E then
warns, naming the format and filter it had to ignore.

So: to let a plugin own the subscriber, opt the entrypoint out.

```rust
r2e::app_main!(MyApp, tracing = false);   // R2E installs nothing at all
```

`launch!(MyApp, tracing = false)` and `#[r2e::main(tracing = false)]` are the
same knob for a custom `main`. Use it whenever you install `Observability`, or
a `Tracing` plugin built from something other than the `tracing:` section.

## OpenTelemetry observability

Enable the `observability` feature and install `Observability` instead of
`Tracing` for distributed traces:

```rust
use r2e::r2e_observability::Observability;

builder
    .plugin(Observability::from_env("my-service"))
    .build_state()
    .await
```

`from_env` reads the standard OTLP endpoint, service-name, protocol, and
sampler variables. With no `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` or
`OTEL_EXPORTER_OTLP_ENDPOINT`, it installs ordinary R2E tracing without an
exporter — the same plugin therefore works locally and deployed, without
conditional subscriber initialization in your code.

`Observability` installs its own subscriber (fmt **plus** the OpenTelemetry
layer) from `App::build`, so it needs the entrypoint to stand down:

```rust
r2e::app_main!(MyApp, tracing = false);
```

Without that, the entrypoint's subscriber is already installed when the plugin
runs, the OTel layer is skipped, and you get logs but **no exported spans**.
R2E says so — "Observability tracing layer skipped" — rather than failing
silently, but the fix is the `tracing = false` above. The same applies to a
custom `#[r2e::main(tracing = false)]` entrypoint.

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

The `RequestIdPlugin` assigns a unique identifier to every request, enabling correlation across log lines and distributed systems.

```rust
AppBuilder::new()
    .plugin(RequestIdPlugin)
    .plugin(Tracing)
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

// Application setup
let builder = AppBuilder::new().load_config::<RootConfig>();

let tracing = Tracing::from_config(builder.r2e_config().unwrap());

builder
    .plugin(RequestIdPlugin)     // Assign request IDs
    .plugin(tracing)             // Configurable tracing
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
- HTTP-level request/response traces (via `Tracing` plugin)

## `Tracing` vs `Observability` plugin

R2E offers two levels of tracing support:

| | `Tracing` / `ConfiguredTracing` | `Observability` |
|---|---|---|
| Crate | `r2e-core` (always available) | `r2e-observability` (feature `observability`) |
| Log subscriber | `tracing_subscriber::fmt` | `tracing_subscriber::fmt` + `tracing-opentelemetry` |
| HTTP trace layer | tower-http `TraceLayer` | tower-http `TraceLayer` + `OtelTraceLayer` |
| Distributed tracing | No | Yes (OTLP export to Jaeger, Tempo, etc.) |
| Context propagation | No | Yes (W3C `traceparent`) |
| Configuration | `TracingConfig` (YAML + builder) | `ObservabilityConfig` (embeds `TracingConfig`) |

Use `Tracing` for local development and simple services. Switch to `Observability` when you need distributed tracing across microservices. Do not install both -- `Observability` already includes the `TraceLayer` and its own log subscriber.

Both `ConfiguredTracing` and `Observability` use `TracingConfig` for subscriber formatting options (format, ansi, thread IDs, etc.). The `Observability` plugin embeds a `TracingConfig` in its `ObservabilityConfig`, so YAML keys live under `observability.tracing.*` instead of `tracing.*`.
