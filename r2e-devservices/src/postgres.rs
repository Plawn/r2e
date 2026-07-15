use testcontainers::{ContainerAsync, ImageExt, ReuseDirective};
use testcontainers_modules::postgres::Postgres;
use tokio::sync::OnceCell;

use crate::common;

/// Default image tag. The testcontainers module's own default
/// (`postgres:11-alpine`) predates arm64 images and fails with
/// `exec format error` on Apple Silicon.
const DEFAULT_TAG: &str = "16-alpine";

/// Container port Postgres listens on.
const CONTAINER_PORT: u16 = 5432;

/// A containerized PostgreSQL instance for tests.
///
/// Credentials are `postgres`/`postgres`, database `postgres` (the
/// testcontainers module defaults).
pub struct DevPostgres {
    /// Keeps the container alive.
    ///
    /// For [`start`](Self::start) the container is removed when this handle
    /// drops (unless `R2E_DEVSERVICES_KEEP` is set). For
    /// [`shared`](Self::shared) the handle lives in a process-wide `static`
    /// and the container is a reused, stable-named instance that deliberately
    /// survives across processes and runs.
    _container: ContainerAsync<Postgres>,
    url: String,
}

impl DevPostgres {
    /// Start a fresh, isolated PostgreSQL container (`postgres:16-alpine`).
    ///
    /// The container is removed when the returned handle drops, unless
    /// `R2E_DEVSERVICES_KEEP` is set (then it is kept alive for inspection).
    ///
    /// # Panics
    ///
    /// Panics if Docker is unavailable or the container fails to start.
    pub async fn start() -> Self {
        Self::start_isolated(DEFAULT_TAG).await
    }

    /// [`start`](Self::start) with a specific `postgres` image tag.
    pub async fn start_with_tag(tag: &str) -> Self {
        Self::start_isolated(tag).await
    }

    /// Isolated, handle-scoped container (`ReuseDirective::Never`).
    async fn start_isolated(tag: &str) -> Self {
        let keep = common::keep_enabled();
        let tag = tag.to_string();
        let make = move || {
            let request = Postgres::default().with_tag(&tag);
            if keep {
                // A reuse container is not reaped on drop, so it survives for
                // post-mortem inspection.
                request.with_reuse(ReuseDirective::Always)
            } else {
                request
            }
        };
        let container = common::start_with_retry("Postgres", make).await;
        Self::from_container(container).await
    }

    /// The shared, cross-process PostgreSQL container, started on first use.
    ///
    /// Uses a stable container name (`r2e-devpostgres-<tag>`) with
    /// `ReuseDirective::Always`, so every test process attaches to the same
    /// container instead of spawning its own. The container is not reaped on
    /// process exit — it stays warm and is reused by the next run, which is
    /// exactly what keeps test suites from accumulating (and leaking) one
    /// container per test binary.
    ///
    /// Tests sharing the container must not assume an empty database —
    /// use per-test schemas/tables or [`start`](Self::start) for isolation.
    pub async fn shared() -> &'static Self {
        static SHARED: OnceCell<DevPostgres> = OnceCell::const_new();
        SHARED
            .get_or_init(|| Self::start_shared(DEFAULT_TAG))
            .await
    }

    /// Reused, stable-named container (`ReuseDirective::Always`).
    async fn start_shared(tag: &str) -> Self {
        let name = common::shared_name("postgres", tag);
        let tag = tag.to_string();
        let make = move || {
            Postgres::default()
                .with_tag(&tag)
                .with_container_name(&name)
                .with_reuse(ReuseDirective::Always)
        };
        let container = common::start_with_retry("Postgres", make).await;
        Self::from_container(container).await
    }

    async fn from_container(container: ContainerAsync<Postgres>) -> Self {
        let host = container
            .get_host()
            .await
            .expect("failed to resolve the Postgres container host")
            .to_string();
        let port = container
            .get_host_port_ipv4(CONTAINER_PORT)
            .await
            .expect("failed to resolve the mapped Postgres port");
        // Guard against attaching to a container whose Postgres is still
        // initializing (relevant on the reuse path).
        common::wait_tcp_ready(host.clone(), port).await;
        let url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
        Self {
            _container: container,
            url,
        }
    }

    /// Connection URL: `postgres://postgres:postgres@{host}:{port}/postgres`.
    pub fn url(&self) -> &str {
        &self.url
    }
}
