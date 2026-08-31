# r2e-prometheus

Prometheus metrics plugin for R2E — HTTP request tracking and `/metrics` endpoint.

## Overview

Collects HTTP request metrics (counts, latency) and exposes them in Prometheus text format. Installs as a Tower layer for zero-configuration request instrumentation.

## Usage

Via the facade crate:

```toml
[dependencies]
r2e = { version = "0.1", features = ["prometheus"] }
```

## Setup

```rust
use r2e::r2e_prometheus::Prometheus;

AppBuilder::new()
    .plugin(Prometheus::new("/metrics"))
    .build_state()
    .await
    .register_controller::<UserController>()
    .serve("0.0.0.0:3000")
    .await;
```

## Metrics

The plugin automatically tracks:

- **Request count** — total HTTP requests by method, path, and status code
- **Request latency** — response time histogram by method and path

Metrics are served at `GET /metrics` in Prometheus text exposition format, ready for scraping.

### Path labels

The `path` label is the matched route template (`/users/{id}`), so cardinality
stays bounded under arbitrary-path scanner traffic. Requests served by a
router *fallback* — SPA/asset serving from `r2e-static`, controller
`#[fallback]` gateway routes, plain 404s — carry no route template and are all
recorded under the single `path="unmatched"` sentinel, even when they return
200. With the `NormalizePath` plugin enabled, trailing-slash requests are
rewritten before routing, so `GET /users/1/` is recorded as `/users/{id}`.

## Modes

The plugin owns three separable things — the HTTP tracking layer, the
`prometheus` registry, and the `/metrics` route. Take as many as you want:

| mode | layer | registry | `/metrics` |
|---|---|---|---|
| `Prometheus::new("/metrics")` (default) | R2E | R2E | R2E |
| `Prometheus::layer_only()` | R2E | R2E | you |
| `MetricsFacade` (feature `metrics-facade`) | R2E | you (`metrics`) | you |

### Layer-only

```rust
.plugin(Prometheus::layer_only())
// or: .plugin(Prometheus::builder().namespace("myapp").without_endpoint().build())
```

R2E still records into its registry — expose it yourself with
`r2e_prometheus::encode_metrics()` (your own route, a push gateway, a sidecar).
Same switch from config, with the usual "builder setting wins" precedence:

```yaml
prometheus:
  expose_endpoint: false
```

### `metrics`-facade backend (feature `metrics-facade`)

For applications already on the [`metrics`](https://docs.rs/metrics) facade with
their own exporter. R2E installs **only** the tracking layer; you keep owning
the recorder, the histogram buckets and the scrape endpoint.

```rust
use r2e::r2e_prometheus::MetricsFacade;

// Install your recorder first (metric descriptions are routed to whatever
// recorder is installed when the plugin builds).
let handle = metrics_exporter_prometheus::PrometheusBuilder::new().install_recorder()?;

AppBuilder::new()
    .plugin(MetricsFacade::builder().namespace("myapp").exclude_path("/health").build())
    // … your own /metrics route rendering `handle`
```

Config keys: `metrics.namespace`, `metrics.exclude_paths`, `metrics.enabled`.
Emitted series are identical to the `prometheus` backend
(`http_requests_total{method,path,status}`,
`http_request_duration_seconds{method,path}`, `http_requests_in_flight`), so
dashboards are portable between the two stacks. Buckets are not configurable
here — with the `metrics` facade the exporter owns bucket layout. R2E's own
`prometheus` registry is never installed in this mode, and
`metrics-exporter-prometheus` is not an R2E dependency: choosing an exporter is
the application's call.

Design rationale: `docs/claude/metrics-stacks.md`.

## License

Apache-2.0
