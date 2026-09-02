---
topic: prometheus
features: prometheus
tokens: ~800
requires: plugins
---

## Prometheus Metrics

### TL;DR

- Enable feature `prometheus`, then install `Prometheus::new("/metrics")` or
  `Prometheus::builder()…build()`: layer + registry + `/metrics` route.
- Emitted series: `http_requests_total` (method/path/status),
  `http_request_duration_seconds`, `http_requests_in_flight`; path labels use the
  matched route template, so cardinality stays bounded.
- Drop noise with `.exclude_path("/health")` / `.exclude_path("/metrics")`.
- To expose the metrics yourself, take `Prometheus::layer_only()` (or
  `.without_endpoint()`, or `prometheus.expose_endpoint: false`) and render with
  `r2e_prometheus::encode_metrics()`; a builder setting wins over file config.
- Already on the `metrics` crate: use `MetricsFacade` (feature `metrics-facade`,
  NOT in `full`) — R2E installs only the tracking layer, you own the recorder,
  the buckets and the endpoint.
- Both backends emit the same names/kinds/labels, so dashboards are portable;
  `metrics-exporter-prometheus` is not an R2E dependency — add it yourself.
- Config keys: `metrics.namespace`, `metrics.exclude_paths`, `metrics.enabled`.

Requires feature: `prometheus`

```rust
# fn __doc(b: AppBuilder) -> impl Sized {
use r2e::r2e_prometheus::Prometheus;

b.plugin(Prometheus::builder()
    .endpoint("/metrics")
    .namespace("myapp")
    .exclude_path("/health")
    .exclude_path("/metrics")
    .build())
# }
```

Metrics: `http_requests_total` (method/path/status), `http_request_duration_seconds`,
`http_requests_in_flight`. Path labels use the matched route template (bounded
cardinality).

The plugin owns three separable things: the HTTP tracking layer, the
`prometheus` registry, and the `/metrics` route. Pick how many of them you want:

```rust,ignore
// 1. Default — layer + registry + /metrics (unchanged).
.plugin(Prometheus::new("/metrics"))

// 2. Layer-only — R2E tracks requests into its registry, YOU expose them
//    (own route, push gateway, sidecar) via `r2e_prometheus::encode_metrics()`.
.plugin(Prometheus::layer_only())
.plugin(Prometheus::builder().namespace("myapp").without_endpoint().build())
// Same switch from YAML (builder setting wins over file config):
//   prometheus:
//     expose_endpoint: false
```

```rust,ignore
// 3. `metrics`-facade backend — feature `metrics-facade` (NOT in `full`).
//    For apps already on the `metrics` crate + their own exporter: R2E installs
//    ONLY the tracking layer, you own the recorder, the buckets and the endpoint.
use r2e::r2e_prometheus::MetricsFacade;

let handle = metrics_exporter_prometheus::PrometheusBuilder::new().install_recorder()?;
AppBuilder::new()
    .plugin(MetricsFacade::builder().namespace("myapp").exclude_path("/health").build())
    // … your own /metrics route rendering `handle`
```

Config: `metrics.namespace`, `metrics.exclude_paths`, `metrics.enabled`. Same
metric names/kinds/labels as the `prometheus` backend, so dashboards are
portable. R2E's `prometheus` registry is never installed in this mode, and
`metrics-exporter-prometheus` is NOT an R2E dependency — the app picks it.
