use testcontainers::ImageExt;
use testcontainers_modules::redis::Redis;

use crate::service::{DevService, DevServiceSpec};

/// Default image repository.
const DEFAULT_NAME: &str = "redis";
/// Default image tag. The testcontainers module's own default (`redis:5.0`)
/// predates arm64 images and fails on Apple Silicon.
const DEFAULT_TAG: &str = "7-alpine";
const CONTAINER_PORT: u16 = 6379;

/// The Docker image backing a [`DevRedis`] container.
///
/// The default is `redis:7-alpine`. Use [`new`](Self::new) for a drop-in
/// replacement or a distribution with extra modules:
///
/// ```ignore
/// let redis = DevRedis::shared_with(RedisImage::new("valkey/valkey", "8-alpine")).await;
/// ```
///
/// The image is part of the shared container's identity, so two different
/// images yield two distinct shared containers.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RedisImage {
    name: String,
    tag: String,
}

impl Default for RedisImage {
    /// `redis:7-alpine`.
    fn default() -> Self {
        Self {
            name: DEFAULT_NAME.to_string(),
            tag: DEFAULT_TAG.to_string(),
        }
    }
}

impl RedisImage {
    /// An image by repository and tag, e.g. `("valkey/valkey", "8-alpine")`.
    ///
    /// The image must speak the Redis protocol on 6379 and log
    /// `Ready to accept connections` when up.
    pub fn new(name: impl Into<String>, tag: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            tag: tag.into(),
        }
    }

    /// The default `redis` repository with a specific tag.
    pub fn with_tag(tag: impl Into<String>) -> Self {
        Self::new(DEFAULT_NAME, tag)
    }

    /// The image repository, e.g. `valkey/valkey`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The image tag, e.g. `8-alpine`.
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// The full image reference, `{name}:{tag}`.
    pub fn reference(&self) -> String {
        format!("{}:{}", self.name, self.tag)
    }

    fn service_spec(&self) -> DevServiceSpec<Redis> {
        let image = self.clone();
        // Kept byte-identical to the pre-parameterization string for the
        // default image, so existing shared containers stay valid.
        let configuration = format!("image={};port={CONTAINER_PORT}", image.reference());
        DevServiceSpec::new("redis", move || {
            Redis::default().with_name(&image.name).with_tag(&image.tag)
        })
        .with_port(CONTAINER_PORT)
        .with_configuration(configuration)
    }
}

/// A containerized Redis instance for tests.
pub struct DevRedis {
    /// The isolated container this handle owns. `None` on the shared path:
    /// there the container belongs to the process-wide registry and outlives
    /// every handle, so the handle is a cheap copy of the connection details.
    _container: Option<DevService>,
    host: String,
    port: u16,
    url: String,
}

impl DevRedis {
    /// Start a fresh, isolated Redis container (`redis:7-alpine`).
    ///
    /// # Panics
    ///
    /// Panics if Docker is unavailable or the container fails to start.
    pub async fn start() -> Self {
        Self::start_with(RedisImage::default()).await
    }

    /// [`start`](Self::start) with a specific `redis` image tag.
    pub async fn start_with_tag(tag: &str) -> Self {
        Self::start_with(RedisImage::with_tag(tag)).await
    }

    /// [`start`](Self::start) with a specific image, repository included.
    pub async fn start_with(image: RedisImage) -> Self {
        let service = DevService::start(image.service_spec()).await;
        let mut handle = Self::describe(&service);
        handle._container = Some(service);
        handle
    }

    /// The cross-process shared Redis container, started on first use.
    ///
    /// The *container* is shared; the returned handle is a cheap owned copy of
    /// its connection details, so dropping it stops nothing.
    pub async fn shared() -> Self {
        Self::shared_with(RedisImage::default()).await
    }

    /// [`shared`](Self::shared) for a specific image.
    ///
    /// One shared container per image, reused across the test binaries of the
    /// session.
    pub async fn shared_with(image: RedisImage) -> Self {
        Self::describe(DevService::shared(image.service_spec()).await)
    }

    /// The connection details of a running container, without owning it.
    fn describe(service: &DevService) -> Self {
        let host = service.host().to_string();
        let port = service.port(CONTAINER_PORT);
        Self {
            _container: None,
            url: format!("redis://{host}:{port}"),
            host,
            port,
        }
    }

    /// Connection URL: `redis://{host}:{port}`.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The host the container is reachable on.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The host port Redis is published on.
    pub fn port(&self) -> u16 {
        self.port
    }
}
