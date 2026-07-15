use testcontainers::{ContainerAsync, ImageExt, ReuseDirective};
use testcontainers_modules::redis::Redis;
use tokio::sync::OnceCell;

use crate::common;

/// Default image tag. The testcontainers module's own default (`redis:5.0`)
/// predates arm64 images and fails on Apple Silicon.
const DEFAULT_TAG: &str = "7-alpine";

/// Container port Redis listens on.
const CONTAINER_PORT: u16 = 6379;

/// A containerized Redis instance for tests.
pub struct DevRedis {
    /// Keeps the container alive.
    ///
    /// For [`start`](Self::start) the container is removed when this handle
    /// drops (unless `R2E_DEVSERVICES_KEEP` is set). For
    /// [`shared`](Self::shared) it is a reused, stable-named instance that
    /// deliberately survives across processes and runs.
    _container: ContainerAsync<Redis>,
    url: String,
}

impl DevRedis {
    /// Start a fresh, isolated Redis container (`redis:7-alpine`).
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

    /// [`start`](Self::start) with a specific `redis` image tag.
    pub async fn start_with_tag(tag: &str) -> Self {
        Self::start_isolated(tag).await
    }

    /// Isolated, handle-scoped container (`ReuseDirective::Never`).
    async fn start_isolated(tag: &str) -> Self {
        let keep = common::keep_enabled();
        let tag = tag.to_string();
        let make = move || {
            let request = Redis::default().with_tag(&tag);
            if keep {
                request.with_reuse(ReuseDirective::Always)
            } else {
                request
            }
        };
        let container = common::start_with_retry("Redis", make).await;
        Self::from_container(container).await
    }

    /// The shared, cross-process Redis container, started on first use.
    ///
    /// Uses a stable container name (`r2e-devredis-<tag>`) with
    /// `ReuseDirective::Always`, so every test process attaches to the same
    /// container instead of spawning its own; it stays warm across runs.
    pub async fn shared() -> &'static Self {
        static SHARED: OnceCell<DevRedis> = OnceCell::const_new();
        SHARED.get_or_init(|| Self::start_shared(DEFAULT_TAG)).await
    }

    /// Reused, stable-named container (`ReuseDirective::Always`).
    async fn start_shared(tag: &str) -> Self {
        let name = common::shared_name("redis", tag);
        let tag = tag.to_string();
        let make = move || {
            Redis::default()
                .with_tag(&tag)
                .with_container_name(&name)
                .with_reuse(ReuseDirective::Always)
        };
        let container = common::start_with_retry("Redis", make).await;
        Self::from_container(container).await
    }

    async fn from_container(container: ContainerAsync<Redis>) -> Self {
        let host = container
            .get_host()
            .await
            .expect("failed to resolve the Redis container host")
            .to_string();
        let port = container
            .get_host_port_ipv4(CONTAINER_PORT)
            .await
            .expect("failed to resolve the mapped Redis port");
        common::wait_tcp_ready(host.clone(), port).await;
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
