# Prometheus Metrics

The `r2e-prometheus` crate records HTTP request metrics for every route and can
serve them on a `/metrics` endpoint for Prometheus to scrape. Labels use the
route template (`/users/{id}`), not the raw URI, so label cardinality stays
bounded.

## Setup

```toml
r2e = { version = "0.15", features = ["prometheus"] }
```

`prometheus` is part of `full`. The `metrics-facade` feature (not in `full`)
adds a second mode, described below.

```rust
use r2e::r2e_prometheus::Prometheus;

AppBuilder::new()
    .plugin(Prometheus::new("/metrics"))
    .build_state()
    .await
    .register_controller::<UserController>()
    .serve_auto()
    .await?;
```

## Emitted metrics

| Metric | Kind | Labels |
|---|---|---|
| `http_requests_total` | counter | `method`, `path`, `status` |
| `http_request_duration_seconds` | histogram | `method`, `path` |
| `http_requests_in_flight` | gauge | — |

`path` is the matched route template, or `unmatched` for requests no route
matched. `method` is one of the standard verbs, or `other`. A namespace prefixes
every name (`myapp_http_requests_total`).

## Three modes

| Constructor | Records requests | Owns the registry | Serves `/metrics` |
|---|---|---|---|
| `Prometheus::new("/metrics")` / `Prometheus::builder()..build()` | yes | yes (`prometheus` crate) | yes |
| `Prometheus::layer_only()` | yes | yes | no (export it yourself) |
| `MetricsFacade` (feature `metrics-facade`) | yes | no (your `metrics` recorder) | no (your route) |

### Builder

```rust
let prometheus = Prometheus::builder()
    .endpoint("/metrics")
    .namespace("myapp")
    .buckets(&[0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0])
    .exclude_paths(&["/health", "/metrics"])
    .build();
```

| Method | Effect |
|---|---|
| `.endpoint(path)` | Path of the scrape endpoint (default `/metrics`) |
| `.without_endpoint()` | Record but do not mount the endpoint |
| `.namespace(ns)` | Prefix for every metric name |
| `.buckets(&[f64])` | Histogram buckets for request duration, in seconds |
| `.exclude_paths(&[..])` / `.exclude_path(p)` | Paths not tracked |
| `.register(Box<dyn Collector>)` / `.register_all(..)` | Add your own collectors at startup |
| `.preset(PrometheusConfig)` | Start from a config struct |

With `layer_only()` or `without_endpoint()`, render the registry yourself with
`r2e::r2e_prometheus::encode_metrics`, or push it.

## Configuration (`prometheus.*`)

```yaml
prometheus:
  endpoint: /metrics
  expose_endpoint: true
  namespace: myapp
  buckets: [0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0]
  exclude_paths: ["/health", "/metrics"]
  enabled: true
```

| Key | Default | Meaning |
|---|---|---|
| `prometheus.enabled` | `true` | `false` turns the plugin off |
| `prometheus.endpoint` | `/metrics` | Scrape path |
| `prometheus.expose_endpoint` | `true` | `false` = record only |
| `prometheus.namespace` | none | Metric name prefix |
| `prometheus.buckets` | built-in | Duration buckets (seconds) |
| `prometheus.exclude_paths` | none | Paths not tracked |

The whole section is optional. For each setting, the order of precedence is:
builder method, then config file, then `preset(..)`, then the default.

## Custom metrics

The plugin provides a `PrometheusRegistry` bean. Register your own collectors on
it, and they appear on the same endpoint:

```rust
use r2e::r2e_prometheus::{prometheus, PrometheusRegistry};

#[derive(Clone)]
pub struct OrderMetrics {
    created: prometheus::IntCounter,
}

#[bean]
impl OrderMetrics {
    pub fn new(registry: PrometheusRegistry) -> Self {
        let created = prometheus::IntCounter::new("orders_created_total", "Orders created")
            .expect("valid metric");
        registry.register(Box::new(created.clone())).expect("unique metric");
        Self { created }
    }

    pub fn order_created(&self) {
        self.created.inc();
    }
}
```

`registry.inner()` returns the underlying `prometheus::Registry`. The
`prometheus` crate is re-exported as `r2e::r2e_prometheus::prometheus`, so you do
not need a separate dependency with a matching version.

## `metrics` facade mode

If the app already uses the [`metrics`](https://docs.rs/metrics) crate with its
own recorder and `/metrics` route, a second Prometheus stack would duplicate
everything. `MetricsFacade` records the same three series into whatever
recorder is installed, and leaves the recorder, buckets and endpoint to the app:

```toml
r2e = { version = "0.15", features = ["metrics-facade"] }
```

```rust
use r2e::r2e_prometheus::MetricsFacade;

// Install your recorder first, so metric descriptions reach it.
let handle = metrics_exporter_prometheus::PrometheusBuilder::new().install_recorder()?;

AppBuilder::new()
    .plugin(MetricsFacade::builder().exclude_path("/health").build())
    // ... your own /metrics route rendering `handle`
```

Its config prefix is `metrics` (`metrics.namespace`, `metrics.exclude_paths`,
`metrics.enabled`). If you are replacing a hand-written metrics middleware,
remove it and add the plugin. Series names stay the same, but `path` becomes
the route template.

## Sharded serving

With [sharded serving](./sharded-serving.md), build a
`r2e::r2e_prometheus::WorkerCollector::new(worker_set)` from the `WorkerSet` bean
and register it on the `PrometheusRegistry`. It exports the worker count and
each worker's lifecycle state (`r2e_worker_*`).

## Testing

The endpoint is a normal route, so a `TestApp` can scrape it:

```rust
#[r2e::test(app = MyApp)]
async fn exposes_metrics(app: TestApp) {
    app.get("/users").send().await.assert_ok();
    let body = app.get("/metrics").send().await.text();
    assert!(body.contains("http_requests_total"));
}
```

## See also

- Design notes: [docs/claude/metrics-stacks.md](https://github.com/plawn/r2e/blob/master/docs/claude/metrics-stacks.md)
- [Observability](./observability.md)
- [Sharded Serving](./sharded-serving.md)
