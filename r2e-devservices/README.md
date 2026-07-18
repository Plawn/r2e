# r2e-devservices

Dev services for R2E tests — Quarkus-style containerized infrastructure on demand.

## Overview

Starts Docker containers (via testcontainers) for databases and services needed by integration tests. Connection URLs are injected into the test app's config automatically.

## Usage

Via the facade crate:

```toml
[dependencies]
r2e = { version = "0.1", features = ["devservices"] }

# Or directly:
[dev-dependencies]
r2e-devservices = { version = "0.1", features = ["postgres"] }
```

### PostgreSQL

```rust
use r2e_devservices::DevPostgres;
use r2e_test::TestApp;

#[tokio::test]
async fn users_are_persisted() {
    let pg = DevPostgres::shared().await;
    let app = TestApp::boot_with::<my_app::MyApp>(|b| {
        b.override_config_value("app.database.url", pg.url())
    })
    .await;
    // ...
}
```

### Redis

```rust
use r2e_devservices::DevRedis;

let redis = DevRedis::shared().await;
// redis.url() → "redis://localhost:32789"
```

## Lifecycle

- `shared()` — reuses one stable container across all test processes. A shared Ryuk reaper removes it after the final process exits.
- `start()` — starts an isolated container whose lifetime follows the returned handle; Ryuk removes it after a crash or forced process termination.
- `R2E_DEVSERVICES_KEEP=1` — disables Ryuk and cleanup for post-mortem inspection.

Ryuk is pinned to `testcontainers/ryuk:0.14.0`. It needs access to the
Docker Unix socket and is started automatically on first use. Its default
reconnection grace period is 10 seconds, allowing consecutive test binaries
to join the same session before cleanup begins.

### Ryuk configuration

| Environment variable | Purpose |
|----------------------|---------|
| `R2E_DEVSERVICES_DOCKER_SOCKET` | Override the host path of the Docker Unix socket |
| `R2E_DEVSERVICES_RYUK_RECONNECTION_TIMEOUT` | Grace period as a Go duration, e.g. `3s` (default `10s`) |
| `R2E_DEVSERVICES_RYUK_PRIVILEGED=1` | Run Ryuk privileged when required by the Docker environment |
| `R2E_DEVSERVICES_SESSION` | Override the workspace-derived cross-process session identity |
| `R2E_DEVSERVICES_KEEP=1` | Disable Ryuk and fallback cleanup |

Remote Docker endpoints without a local Unix socket are not currently
supported by the embedded Ryuk integration.

## Feature flags

| Feature | Description |
|---------|-------------|
| `postgres` | `DevPostgres` — containerized PostgreSQL |
| `redis` | `DevRedis` — containerized Redis |
| `openfga` | `DevOpenFga` — containerized OpenFGA (exposes `grpc_endpoint()` / `http_endpoint()`) |

## License

Apache-2.0
