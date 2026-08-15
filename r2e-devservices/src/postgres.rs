use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

use testcontainers::core::ContainerRequest;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt, ReuseDirective};
use testcontainers_modules::postgres::Postgres;
use tokio::sync::OnceCell;

use crate::{common, ryuk};

/// Default image repository.
const DEFAULT_NAME: &str = "postgres";
/// Default image tag. The testcontainers module's own default
/// (`postgres:11-alpine`) predates arm64 images and fails with
/// `exec format error` on Apple Silicon.
const DEFAULT_TAG: &str = "16-alpine";
const CONTAINER_PORT: u16 = 5432;

/// The Docker image backing a [`DevPostgres`] container.
///
/// The default is `postgres:16-alpine`. Use [`new`](Self::new) to run a
/// Postgres distribution that ships extra extensions:
///
/// ```ignore
/// let pg = DevPostgres::shared_with_image(PostgresImage::new("pgvector/pgvector", "pg18")).await;
/// ```
///
/// The image is part of the shared container's identity, so two different
/// images yield two distinct shared containers.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PostgresImage {
    name: String,
    tag: String,
}

impl Default for PostgresImage {
    /// `postgres:16-alpine`.
    fn default() -> Self {
        Self {
            name: DEFAULT_NAME.to_string(),
            tag: DEFAULT_TAG.to_string(),
        }
    }
}

impl PostgresImage {
    /// An image by repository and tag, e.g. `("pgvector/pgvector", "pg18")`.
    ///
    /// The image must behave like the official `postgres` one: same default
    /// credentials, same port, and a `database system is ready` readiness log.
    pub fn new(name: impl Into<String>, tag: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            tag: tag.into(),
        }
    }

    /// The default `postgres` repository with a specific tag.
    pub fn with_tag(tag: impl Into<String>) -> Self {
        Self::new(DEFAULT_NAME, tag)
    }

    /// The image repository, e.g. `pgvector/pgvector`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The image tag, e.g. `pg18`.
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// The full image reference, `{name}:{tag}`.
    pub fn reference(&self) -> String {
        format!("{}:{}", self.name, self.tag)
    }

    /// Every input that affects the container's identity.
    fn configuration(&self) -> String {
        format!(
            "image={};port={CONTAINER_PORT};user=postgres;password=postgres;database=postgres",
            self.reference()
        )
    }

    /// The base container request for this image.
    fn request(&self) -> ContainerRequest<Postgres> {
        Postgres::default()
            .with_name(&self.name)
            .with_tag(&self.tag)
    }
}

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
        Self::start_with_image(PostgresImage::default()).await
    }

    /// [`start`](Self::start) with a specific `postgres` image tag.
    pub async fn start_with_tag(tag: &str) -> Self {
        Self::start_with_image(PostgresImage::with_tag(tag)).await
    }

    /// [`start`](Self::start) with a specific image, repository included.
    pub async fn start_with_image(image: PostgresImage) -> Self {
        ryuk::ensure_lease().await;
        let request = common::label_isolated(image.request(), "postgres", &image.configuration());
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
        Self::shared_with_image(PostgresImage::default()).await
    }

    /// [`shared`](Self::shared) for a specific image.
    ///
    /// Each image gets its own shared container: same image ⇒ same container,
    /// different image ⇒ a separate one, within the process and across the
    /// test binaries of the session.
    pub async fn shared_with_image(image: PostgresImage) -> &'static Self {
        static SHARED: OnceLock<Mutex<HashMap<String, &'static OnceCell<DevPostgres>>>> =
            OnceLock::new();

        // One cell per image, leaked to hand out `&'static` for the process's
        // lifetime — the container lives as long as the cell that owns it.
        let cell = {
            let mut cells = SHARED
                .get_or_init(Mutex::default)
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            *cells
                .entry(image.reference())
                .or_insert_with(|| Box::leak(Box::new(OnceCell::const_new())))
        };
        cell.get_or_init(|| Self::start_shared(image)).await
    }

    async fn start_shared(image: PostgresImage) -> Self {
        ryuk::ensure_lease().await;
        let identity = common::SharedIdentity::new("postgres", &image.configuration());
        common::cleanup(&identity).await;

        let container = common::start_with_retry("Postgres", || {
            identity.label(
                image
                    .request()
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
