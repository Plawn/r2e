use std::any::Any;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

use testcontainers::core::ContainerRequest;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, Image, ImageExt, ReuseDirective};
use tokio::sync::OnceCell;

use crate::{common, ryuk};

/// How to start a containerized dev service.
///
/// This is the generic form behind [`DevPostgres`](crate::DevPostgres) &
/// friends: any testcontainers [`Image`] — a module from
/// `testcontainers-modules`, a `GenericImage`, or your own `Image` impl — can
/// join the R2E test session and get the same labelling, Ryuk reaping and
/// cross-process sharing.
///
/// ```ignore
/// use r2e_devservices::{DevService, DevServiceSpec};
/// use testcontainers_modules::clickhouse::ClickHouse;
///
/// let spec = DevServiceSpec::new("clickhouse", || ClickHouse::default().into()).with_port(8123);
/// let clickhouse = DevService::shared(spec).await;
/// let url = format!("http://{}", clickhouse.endpoint(8123));
/// ```
///
/// The request is built by a closure rather than passed by value because
/// `ContainerRequest` is not `Clone` and a contended start is retried.
pub struct DevServiceSpec<I: Image> {
    service: String,
    ports: Vec<u16>,
    configuration: Option<String>,
    request: Box<dyn Fn() -> ContainerRequest<I> + Send + Sync>,
}

impl<I: Image + 'static> DevServiceSpec<I> {
    /// A service named `service` (the label and container-name segment, e.g.
    /// `"clickhouse"`), started from `request`.
    ///
    /// The name separates services from each other in the shared-container
    /// registry and in the fallback cleanup — keep it stable and distinct.
    pub fn new(
        service: impl Into<String>,
        request: impl Fn() -> ContainerRequest<I> + Send + Sync + 'static,
    ) -> Self {
        Self {
            service: service.into(),
            ports: Vec::new(),
            configuration: None,
            request: Box::new(request),
        }
    }

    /// Publish and resolve a container port, readable back with
    /// [`DevService::port`].
    ///
    /// On the shared path each declared port is also probed before the service
    /// is handed out, since a reused container replays no readiness log.
    pub fn with_port(mut self, container_port: u16) -> Self {
        self.ports.push(container_port);
        self
    }

    /// Override the configuration string identifying the shared container.
    ///
    /// It is fingerprinted into the container name and the
    /// `dev.r2e.devservices.config` label: two specs with the same string share
    /// one container, two different strings get one each. The derived default
    /// covers the image and the declared ports, so **override this whenever
    /// something else must separate two containers** — credentials, env vars, a
    /// command. This replaces the default rather than extending it.
    pub fn with_configuration(mut self, configuration: impl Into<String>) -> Self {
        self.configuration = Some(configuration.into());
        self
    }

    /// The configuration string: the override, or `image=…;port=…` derived
    /// from the request.
    fn configuration(&self) -> String {
        if let Some(configuration) = &self.configuration {
            return configuration.clone();
        }
        let mut configuration = format!("image={}", (self.request)().descriptor());
        for port in &self.ports {
            configuration.push_str(&format!(";port={port}"));
        }
        configuration
    }
}

/// A running dev-service container.
///
/// Obtained from [`DevService::start`] (isolated, tied to the handle) or
/// [`DevService::shared`] (one container per configuration, reused across the
/// test binaries of the session).
pub struct DevService {
    /// Owns the isolated container, or references the reusable shared one.
    /// Type-erased so one registry can hold every service's containers.
    _container: Box<dyn Any + Send + Sync>,
    host: String,
    ports: HashMap<u16, u16>,
}

impl DevService {
    /// Start a fresh, isolated container. Its normal lifetime follows the
    /// returned handle, with Ryuk as the crash/`SIGKILL` fallback.
    ///
    /// # Panics
    ///
    /// Panics if Docker is unavailable or the container fails to start.
    pub async fn start<I: Image + 'static>(spec: DevServiceSpec<I>) -> Self {
        ryuk::ensure_lease().await;
        let request =
            common::label_isolated((spec.request)(), &spec.service, &spec.configuration());
        let container = request.start().await.unwrap_or_else(|error| {
            panic!(
                "failed to start the {} dev service — is Docker running?: {error}",
                spec.service
            )
        });
        Self::resolve(container, &spec, false).await
    }

    /// The cross-process shared container for this configuration, started on
    /// first use.
    ///
    /// Tests sharing a container must not assume exclusive state — namespace
    /// per test, or use [`start`](Self::start) for isolation.
    ///
    /// # Panics
    ///
    /// Panics if Docker is unavailable or the container fails to start.
    pub async fn shared<I: Image + 'static>(spec: DevServiceSpec<I>) -> &'static Self {
        // One cell per (service, configuration), leaked to hand out `&'static`
        // for the process's lifetime — the container lives as long as the cell
        // that owns it. A single registry serves every service because the
        // container type is erased.
        static SHARED: OnceLock<Mutex<HashMap<String, &'static OnceCell<DevService>>>> =
            OnceLock::new();

        let identity = common::SharedIdentity::new(&spec.service, &spec.configuration());
        let cell = {
            let mut cells = SHARED
                .get_or_init(Mutex::default)
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            *cells
                .entry(identity.name().to_string())
                .or_insert_with(|| Box::leak(Box::new(OnceCell::const_new())))
        };
        cell.get_or_init(|| Self::start_shared(spec, identity))
            .await
    }

    async fn start_shared<I: Image + 'static>(
        spec: DevServiceSpec<I>,
        identity: common::SharedIdentity,
    ) -> Self {
        ryuk::ensure_lease().await;
        common::cleanup(&identity).await;

        let container = common::start_with_retry(&spec.service, || {
            identity.label(
                (spec.request)()
                    .with_container_name(identity.name())
                    .with_reuse(ReuseDirective::Always),
            )
        })
        .await;
        Self::resolve(container, &spec, true).await
    }

    /// Resolve the published ports, waiting for them when the container may
    /// have been reused (its readiness log is not replayed).
    async fn resolve<I: Image + 'static>(
        container: ContainerAsync<I>,
        spec: &DevServiceSpec<I>,
        wait: bool,
    ) -> Self {
        let service = &spec.service;
        let host = container
            .get_host()
            .await
            .unwrap_or_else(|error| panic!("cannot resolve the {service} container host: {error}"))
            .to_string();

        let mut ports = HashMap::with_capacity(spec.ports.len());
        for &container_port in &spec.ports {
            let host_port = container
                .get_host_port_ipv4(container_port)
                .await
                .unwrap_or_else(|error| {
                    panic!("cannot resolve the mapped {service} port {container_port}: {error}")
                });
            if wait {
                common::wait_tcp_ready(&host, host_port, service).await;
            }
            ports.insert(container_port, host_port);
        }

        Self {
            _container: Box::new(container),
            host,
            ports,
        }
    }

    /// The host the container is reachable on.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The host port `container_port` is published on.
    ///
    /// # Panics
    ///
    /// Panics if the port was not declared with
    /// [`with_port`](DevServiceSpec::with_port).
    pub fn port(&self, container_port: u16) -> u16 {
        self.ports.get(&container_port).copied().unwrap_or_else(|| {
            panic!("port {container_port} was not declared on the dev service spec")
        })
    }

    /// `{host}:{port}` for a declared container port.
    ///
    /// # Panics
    ///
    /// Panics if the port was not declared with
    /// [`with_port`](DevServiceSpec::with_port).
    pub fn endpoint(&self, container_port: u16) -> String {
        format!("{}:{}", self.host, self.port(container_port))
    }
}
