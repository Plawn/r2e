use testcontainers::core::ContainerRequest;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt, ReuseDirective};
use testcontainers_modules::postgres::Postgres;
use tokio::sync::OnceCell;

use crate::{common, ryuk};

/// Default image tag. The testcontainers module's own default
/// (`postgres:11-alpine`) predates arm64 images and fails with
/// `exec format error` on Apple Silicon.
const DEFAULT_TAG: &str = "16-alpine";
const CONTAINER_PORT: u16 = 5432;

/// A containerized PostgreSQL instance for tests.
///
/// Credentials are `postgres`/`postgres`, database `postgres` (the
/// testcontainers module defaults).
pub struct DevPostgres {
    /// Owns an isolated container, or references the reusable shared container.
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
        Self::start_with_configuration(DEFAULT_TAG).await
    }

    /// [`start`](Self::start) with a specific `postgres` image tag.
    pub async fn start_with_tag(tag: &str) -> Self {
        Self::start_with_configuration(tag).await
    }

    async fn start_with_configuration(tag: &str) -> Self {
        ryuk::ensure_lease().await;
        let configuration = format!(
            "image=postgres:{tag};port={CONTAINER_PORT};user=postgres;password=postgres;database=postgres"
        );
        let request = common::label_isolated(
            Postgres::default().with_tag(tag),
            "postgres",
            &configuration,
        );
        Self::start_request(request).await
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
            .get_host_port_ipv4(CONTAINER_PORT)
            .await
            .expect("failed to resolve the mapped Postgres port");
        let url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
        Self {
            _container: container,
            url,
        }
    }

    /// The cross-process shared PostgreSQL container, started on first use.
    ///
    /// Tests sharing the container must not assume an empty database —
    /// use per-test schemas/tables or [`start`](Self::start) for isolation.
    pub async fn shared() -> &'static Self {
        static SHARED: OnceCell<DevPostgres> = OnceCell::const_new();
        SHARED.get_or_init(Self::start_shared).await
    }

    async fn start_shared() -> Self {
        ryuk::ensure_lease().await;
        let configuration = format!(
            "image=postgres:{DEFAULT_TAG};port={CONTAINER_PORT};user=postgres;password=postgres;database=postgres"
        );
        let identity = common::SharedIdentity::new("postgres", &configuration);
        common::cleanup(&identity).await;

        let container = common::start_with_retry("Postgres", || {
            identity.label(
                Postgres::default()
                    .with_tag(DEFAULT_TAG)
                    .with_container_name(identity.name())
                    .with_reuse(ReuseDirective::Always),
            )
        })
        .await;
        let host = container
            .get_host()
            .await
            .expect("failed to resolve the Postgres container host")
            .to_string();
        let port = container
            .get_host_port_ipv4(CONTAINER_PORT)
            .await
            .expect("failed to resolve the mapped Postgres port");
        common::wait_tcp_ready(&host, port, "Postgres").await;
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
