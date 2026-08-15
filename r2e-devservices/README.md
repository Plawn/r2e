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

A different image (repository *and* tag) works too — useful for distributions
that ship extra extensions — as do custom credentials. Everything in the spec
is part of the container's identity, so each distinct spec gets its own shared
container:

```rust
use r2e_devservices::{DevPostgres, PostgresImage, PostgresSpec};

let pg = DevPostgres::shared_with(PostgresImage::new("pgvector/pgvector", "pg18")).await;
sqlx::query("CREATE EXTENSION IF NOT EXISTS vector").execute(&pool).await?;

let app_db = DevPostgres::shared_with(
    PostgresSpec::default()
        .with_user("app")
        .with_password("s3cret")
        .with_database("appdb"),
)
.await;
// app_db.url() → "postgres://app:s3cret@localhost:32771/appdb"
```

`PostgresSpec::default()` is `postgres:16-alpine` with `postgres`/`postgres`
on database `postgres`; a `PostgresImage` converts into a spec, so either can
be passed. The image must speak Postgres on port 5432 and honour the official
image's `POSTGRES_USER`/`POSTGRES_PASSWORD`/`POSTGRES_DB` variables.
`start_with` is the isolated counterpart.

### Redis

```rust
use r2e_devservices::{DevRedis, RedisImage};

let redis = DevRedis::shared().await;
// redis.url() → "redis://localhost:32789"

let valkey = DevRedis::shared_with(RedisImage::new("valkey/valkey", "8-alpine")).await;
```

### Any other service

`DevService` is the generic form of all of the above: it gives any
testcontainers `Image` the same labelling, Ryuk reaping and cross-process
sharing. A service R2E ships no wrapper for lives entirely on your side:

```rust
use r2e_devservices::{DevService, DevServiceSpec};
use r2e_devservices::testcontainers_modules::clickhouse::ClickHouse;

let clickhouse = DevService::shared(
    DevServiceSpec::new("clickhouse", || ClickHouse::default().into()).with_port(8123),
)
.await;
let url = format!("http://{}", clickhouse.endpoint(8123));
```

`testcontainers` and `testcontainers_modules` are re-exported so your spec
builds against the exact versions this crate uses — a mismatched one produces
a different `ContainerRequest` type and will not compile. `GenericImage` and
your own `Image` impls work the same way.

Two specs share a container when their *configuration string* matches. It is
derived from the image and declared ports; use `with_configuration` when
something else must separate two containers (env vars, a command, credentials
— which is exactly what `PostgresSpec` does):

```rust
DevServiceSpec::new("clickhouse", move || image_for(&user))
    .with_port(8123)
    .with_configuration(format!("image=clickhouse:25;port=8123;user={user}"))
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
| *(none)* | `DevService` / `DevServiceSpec` — any image, always available |
| `postgres` | `DevPostgres` — containerized PostgreSQL |
| `redis` | `DevRedis` — containerized Redis |
| `openfga` | `DevOpenFga` — containerized OpenFGA (exposes `grpc_endpoint()` / `http_endpoint()`) |

## License

Apache-2.0
