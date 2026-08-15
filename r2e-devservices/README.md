# r2e-devservices

Dev services for R2E tests — Quarkus-style containerized infrastructure on demand.

## Overview

Starts Docker containers (via testcontainers) for databases and services needed by integration tests. Connection URLs are injected into the test app's config automatically.

## Usage

```toml
[dev-dependencies]
r2e-devservices = { version = "0.1", features = ["postgres"] }
```

The crate is not re-exported through the `r2e` facade: dev services belong to
`[dev-dependencies]`, never to the binary.

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
let pool = sqlx::PgPool::connect(pg.url()).await?;
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
use r2e_devservices::testcontainers::core::{IntoContainerPort, WaitFor};
use r2e_devservices::testcontainers::{GenericImage, ImageExt};
use r2e_devservices::{DevService, DevServiceSpec};

let clickhouse = DevService::shared(
    DevServiceSpec::new("clickhouse", || {
        GenericImage::new("clickhouse/clickhouse-server", "24.8-alpine")
            .with_exposed_port(8123.tcp())
            .with_wait_for(WaitFor::message_on_either_std("Ready for connections"))
            .into()
    })
    .with_port(8123),
)
.await;
let url = format!("http://{}", clickhouse.endpoint(8123));
```

`testcontainers` and `testcontainers_modules` are re-exported so your spec
builds against the exact versions this crate uses — a mismatched one produces
a different `ContainerRequest` type and will not compile. `GenericImage` and
your own `Image` impls work the same way. The ready-made module images
(`ClickHouse`, `Kafka`, `Mongo`, …) each sit behind their own feature: add
`testcontainers-modules = { version = "0.15", features = ["clickhouse"] }` to
your own `[dev-dependencies]` and Cargo unifies it with the re-export.

`with_port` resolves a port the *image* exposes (`with_exposed_port`,
`Image::expose_ports`, or an `EXPOSE` in the Dockerfile) — testcontainers
publishes those on random host ports; declaring one here does not add one.

Two specs share a container when their identity matches, and that identity is
derived from the request itself: image, env vars (in override order, so the
*effective* value counts), labels, command, entrypoint, mounts, copied files,
network, user, declared ports. So a different image *or* different credentials
get their own container with nothing to declare — that is all `PostgresSpec`
does. Fields Docker treats as a set (exposed ports, capabilities) are sorted
first, so declaration order never splits a container in two. For what the
request cannot express (data seeded after start, the contents of a file copied
by path, an ulimit, a host-config closure), append to the key:

```rust
DevServiceSpec::new("clickhouse", request)
    .with_port(8123)
    .with_discriminator("seeded-fixtures-v2")
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
