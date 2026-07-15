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
    let app = TestApp::boot_with(my_app::app, |b| {
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

- `shared()` — starts the container **once per test process**, reused across all tests. Testcontainers' reaper cleans up after the process exits.
- `start()` — starts an isolated container whose lifetime follows the returned handle.

## Feature flags

| Feature | Description |
|---------|-------------|
| `postgres` | `DevPostgres` — containerized PostgreSQL |
| `redis` | `DevRedis` — containerized Redis |

## License

Apache-2.0
