use testcontainers::core::ContainerRequest;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt, ReuseDirective};
use testcontainers_modules::redis::Redis;
use tokio::sync::OnceCell;

use crate::{common, ryuk};

/// Default image tag. The testcontainers module's own default (`redis:5.0`)
/// predates arm64 images and fails on Apple Silicon.
const DEFAULT_TAG: &str = "7-alpine";
const CONTAINER_PORT: u16 = 6379;

/// A containerized Redis instance for tests.
pub struct DevRedis {
    /// Owns an isolated container, or references the reusable shared container.
    _container: ContainerAsync<Redis>,
    url: String,
}

impl DevRedis {
    /// Start a fresh, isolated Redis container (`redis:7-alpine`).
    ///
    /// # Panics
    ///
    /// Panics if Docker is unavailable or the container fails to start.
    pub async fn start() -> Self {
        Self::start_with_configuration(DEFAULT_TAG).await
    }

    /// [`start`](Self::start) with a specific `redis` image tag.
    pub async fn start_with_tag(tag: &str) -> Self {
        Self::start_with_configuration(tag).await
    }

    async fn start_with_configuration(tag: &str) -> Self {
        ryuk::ensure_lease().await;
        let configuration = format!("image=redis:{tag};port={CONTAINER_PORT}");
        let request =
            common::label_isolated(Redis::default().with_tag(tag), "redis", &configuration);
        Self::start_request(request).await
    }

    async fn start_request(request: ContainerRequest<Redis>) -> Self {
        let container = request
            .start()
            .await
            .expect("failed to start the Redis dev service — is Docker running?");
        let host = container
            .get_host()
            .await
            .expect("failed to resolve the Redis container host");
        let port = container
            .get_host_port_ipv4(CONTAINER_PORT)
            .await
            .expect("failed to resolve the mapped Redis port");
        let url = format!("redis://{host}:{port}");
        Self {
            _container: container,
            url,
        }
    }

    /// The cross-process shared Redis container, started on first use.
    pub async fn shared() -> &'static Self {
        static SHARED: OnceCell<DevRedis> = OnceCell::const_new();
        SHARED.get_or_init(Self::start_shared).await
    }

    async fn start_shared() -> Self {
        ryuk::ensure_lease().await;
        let configuration = format!("image=redis:{DEFAULT_TAG};port={CONTAINER_PORT}");
        let identity = common::SharedIdentity::new("redis", &configuration);
        common::cleanup(&identity).await;

        let container = common::start_with_retry("Redis", || {
            identity.label(
                Redis::default()
                    .with_tag(DEFAULT_TAG)
                    .with_container_name(identity.name())
                    .with_reuse(ReuseDirective::Always),
            )
        })
        .await;
        let host = container
            .get_host()
            .await
            .expect("failed to resolve the Redis container host")
            .to_string();
        let port = container
            .get_host_port_ipv4(CONTAINER_PORT)
            .await
            .expect("failed to resolve the mapped Redis port");
        common::wait_tcp_ready(&host, port, "Redis").await;
        let url = format!("redis://{host}:{port}");
        Self {
            _container: container,
            url,
        }
    }

    /// Connection URL: `redis://{host}:{port}`.
    pub fn url(&self) -> &str {
        &self.url
    }
}
