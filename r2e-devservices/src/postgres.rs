use testcontainers::core::ContainerRequest;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::postgres::Postgres;
use tokio::sync::OnceCell;

/// Default image tag. The testcontainers module's own default
/// (`postgres:11-alpine`) predates arm64 images and fails with
/// `exec format error` on Apple Silicon.
const DEFAULT_TAG: &str = "16-alpine";

/// A containerized PostgreSQL instance for tests.
///
/// Credentials are `postgres`/`postgres`, database `postgres` (the
/// testcontainers module defaults).
pub struct DevPostgres {
    /// Keeps the container alive; it is stopped when this handle drops
    /// (or, for [`shared`](Self::shared), when the test process exits).
    _container: ContainerAsync<Postgres>,
    url: String,
}

impl DevPostgres {
    /// Start a fresh, isolated PostgreSQL container (`postgres:16-alpine`).
    ///
    /// # Panics
    ///
    /// Panics if Docker is unavailable or the container fails to start.
    pub async fn start() -> Self {
        Self::start_request(Postgres::default().with_tag(DEFAULT_TAG)).await
    }

    /// [`start`](Self::start) with a specific `postgres` image tag.
    pub async fn start_with_tag(tag: &str) -> Self {
        Self::start_request(Postgres::default().with_tag(tag)).await
    }

    async fn start_request(request: ContainerRequest<Postgres>) -> Self {
        let container = request
            .start()
            .await
            .expect("failed to start the Postgres dev service — is Docker running?");
        let host = container
            .get_host()
            .await
            .expect("failed to resolve the Postgres container host");
        let port = container
            .get_host_port_ipv4(5432)
            .await
            .expect("failed to resolve the mapped Postgres port");
        let url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
        Self {
            _container: container,
            url,
        }
    }

    /// The process-wide shared PostgreSQL container, started on first use.
    ///
    /// Tests sharing the container must not assume an empty database —
    /// use per-test schemas/tables or [`start`](Self::start) for isolation.
    pub async fn shared() -> &'static Self {
        static SHARED: OnceCell<DevPostgres> = OnceCell::const_new();
        SHARED.get_or_init(Self::start).await
    }

    /// Connection URL: `postgres://postgres:postgres@{host}:{port}/postgres`.
    pub fn url(&self) -> &str {
        &self.url
    }
}
