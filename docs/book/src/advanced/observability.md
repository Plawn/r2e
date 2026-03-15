# Observability

R2E provides built-in observability through request tracing, request IDs, and metric interceptors. These tools work together to give visibility into your application's behavior.

## Tracing plugin

The `Tracing` plugin initializes structured logging and adds HTTP-level trace spans to every request.

```rust
use r2e::prelude::*;

AppBuilder::new()
    .build_state::<AppState, _, _>()
    .await
    .with(Tracing)
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
RUST_LOG="info,tower_http=debug"

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
| `filter` | String | `"info,tower_http=debug"` | `EnvFilter` directive. `RUST_LOG` env var takes priority. |
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
    .build_state::<AppState, _, _>()
    .await
    .with(Tracing::configured(tracing_config))
    .serve("0.0.0.0:3000")
    .await;
```

### Loading from R2eConfig

When using `load_config`, you can read `TracingConfig` directly from your YAML:

```rust
AppBuilder::new()
    .load_config::<RootConfig>()
    .build_state::<AppState, _, _>()
    .await
    .with(Tracing::from_config(&app.r2e_config().unwrap()))
    .serve("0.0.0.0:3000")
    .await;
```

`Tracing::from_config()` reads the `tracing.*` keys from `R2eConfig`.

### Using `#[r2e::main(tracing = false)]`

By default, `#[r2e::main]` calls `init_tracing()` before config is loaded — so there is no config available yet. If you want configurable tracing:

1. Disable the default tracing: `#[r2e::main(tracing = false)]`
2. Load config first, then install the configured tracing plugin

## RequestId plugin

The `RequestIdPlugin` assigns a unique identifier to every request, enabling correlation across log lines and distributed systems.

```rust
AppBuilder::new()
    .build_state::<AppState, _, _>()
    .await
    .with(RequestIdPlugin)
    .with(Tracing)
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

#[derive(Controller)]
#[controller(path = "/api", state = AppState)]
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
AppBuilder::new()
    .load_config::<RootConfig>()
    .build_state::<AppState, _, _>()
    .await
    .with(RequestIdPlugin)                                // Assign request IDs
    .with(Tracing::from_config(&app.r2e_config().unwrap()))  // Configurable tracing
    .with(Health)                                         // Health check endpoint
    .register::<ApiController>()
    .serve("0.0.0.0:3000")
    .await;
```

```rust
#[derive(Controller)]
#[controller(path = "/users", state = AppState)]
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
