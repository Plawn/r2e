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
    .build_state::<AppState, _, _>()
    .await
    .with(Prometheus)
    .register_controller::<UserController>()
    .serve("0.0.0.0:3000")
    .await;
```

## Metrics

The plugin automatically tracks:

- **Request count** — total HTTP requests by method, path, and status code
- **Request latency** — response time histogram by method and path

Metrics are served at `GET /metrics` in Prometheus text exposition format, ready for scraping.

## License

Apache-2.0
