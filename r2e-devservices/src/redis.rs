use testcontainers::core::ContainerRequest;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::redis::Redis;
use tokio::sync::OnceCell;

/// Default image tag. The testcontainers module's own default (`redis:5.0`)
/// predates arm64 images and fails on Apple Silicon.
const DEFAULT_TAG: &str = "7-alpine";

/// A containerized Redis instance for tests.
pub struct DevRedis {
    /// Keeps the container alive; it is stopped when this handle drops
    /// (or, for [`shared`](Self::shared), when the test process exits).
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
        Self::start_request(Redis::default().with_tag(DEFAULT_TAG)).await
    }

    /// [`start`](Self::start) with a specific `redis` image tag.
    pub async fn start_with_tag(tag: &str) -> Self {
        Self::start_request(Redis::default().with_tag(tag)).await
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
            .get_host_port_ipv4(6379)
            .await
            .expect("failed to resolve the mapped Redis port");
        let url = format!("redis://{host}:{port}");
        Self {
            _container: container,
            url,
        }
    }

    /// The process-wide shared Redis container, started on first use.
    pub async fn shared() -> &'static Self {
        static SHARED: OnceCell<DevRedis> = OnceCell::const_new();
        SHARED.get_or_init(Self::start).await
    }

    /// Connection URL: `redis://{host}:{port}`.
    pub fn url(&self) -> &str {
        &self.url
    }
}
