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
/// use r2e_devservices::testcontainers::core::{IntoContainerPort, WaitFor};
/// use r2e_devservices::testcontainers::{GenericImage, ImageExt};
/// use r2e_devservices::{DevService, DevServiceSpec};
///
/// let spec = DevServiceSpec::new("clickhouse", || {
///     GenericImage::new("clickhouse/clickhouse-server", "24.8-alpine")
///         .with_exposed_port(8123.tcp())
///         .with_wait_for(WaitFor::message_on_either_std("Ready for connections"))
///         .into()
/// })
/// .with_port(8123);
///
/// let clickhouse = DevService::shared(spec).await;
/// let url = format!("http://{}", clickhouse.endpoint(8123));
/// ```
///
/// The request is built by a closure rather than passed by value because
/// `ContainerRequest` is not `Clone` and a contended start is retried.
pub struct DevServiceSpec<I: Image> {
    service: String,
    ports: Vec<u16>,
    discriminator: Option<String>,
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
            discriminator: None,
            request: Box::new(request),
        }
    }

    /// Resolve a container port, readable back with [`DevService::port`].
    ///
    /// The port must be *exposed by the image* — `GenericImage::with_exposed_port`,
    /// `Image::expose_ports`, or an `EXPOSE` in the Dockerfile; testcontainers
    /// then publishes it on a random host port. This declares which of those
    /// published ports the handle should resolve, it does not add one.
    /// TCP only.
    ///
    /// On the shared path each declared port is also probed before the service
    /// is handed out, since a reused container replays no readiness log.
    pub fn with_port(mut self, container_port: u16) -> Self {
        self.ports.push(container_port);
        self
    }

    /// Split the shared container further, on something the request does not
    /// carry.
    ///
    /// The identity is already derived from the request itself (see
    /// [`configuration`](Self::configuration)), so images, credentials, env
    /// vars, commands and mounts separate containers on their own. Reach for
    /// this only for what stays *outside* the request — data seeded after
    /// start, a host-config modifier closure, an ulimit — or to force two
    /// otherwise identical containers apart:
    ///
    /// ```ignore
    /// let spec = DevServiceSpec::new("kafka", request).with_discriminator("seeded-topics-v2");
    /// ```
    ///
    /// It is appended to the derived identity, never replaces it: this can only
    /// ever split containers, never merge two different ones.
    pub fn with_discriminator(mut self, discriminator: impl Into<String>) -> Self {
        self.discriminator = Some(discriminator.into());
        self
    }

    /// The canonical description of everything that shapes the container.
    ///
    /// Fingerprinted into the container name and the
    /// `dev.r2e.devservices.config` label: same string ⇒ same shared container.
    /// It covers the image type and reference, the declared ports, and every
    /// request field testcontainers exposes — env vars, command, entrypoint,
    /// mounts, copied files, network, user, and the rest — so two requests that
    /// differ in any of them get two containers.
    ///
    /// What it cannot see: values testcontainers keeps private (ulimits, the
    /// host-config modifier closure) and anything applied *after* start (seeded
    /// data). Separate those with
    /// [`with_discriminator`](Self::with_discriminator).
    #[doc(hidden)]
    pub fn configuration(&self) -> String {
        let request = (self.request)();
        let mut ports = self.ports.clone();
        ports.sort_unstable();
        // Some `Image` impls hold their env vars in a `HashMap` (the Postgres
        // module does), so the iteration order is not stable across processes:
        // sort before digesting or the same spec fingerprints differently in
        // two test binaries.
        let mut env: Vec<String> = request
            .env_vars()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        env.sort();

        let mut configuration = Configuration::default();
        configuration.field("type", std::any::type_name::<I>());
        configuration.field("image", request.descriptor());
        configuration.list("port", ports.iter().map(u16::to_string));
        configuration.list("expose", request.expose_ports().iter().map(debug));
        configuration.list("map", request.ports().into_iter().flatten().map(debug));
        configuration.field("entrypoint", request.entrypoint().unwrap_or_default());
        configuration.list("cmd", request.cmd());
        configuration.list("env", env);
        configuration.list(
            "host",
            request
                .hosts()
                .map(|(name, host)| format!("{name}={host:?}")),
        );
        configuration.list("mount", request.mounts().map(debug));
        configuration.list("copy", request.copy_to_sources().map(debug));
        configuration.field("network", request.network().clone().unwrap_or_default());
        configuration.field("hostname", request.hostname().unwrap_or_default());
        configuration.field("platform", request.platform().clone().unwrap_or_default());
        configuration.field("workdir", request.working_dir().unwrap_or_default());
        configuration.field("user", request.user().unwrap_or_default());
        configuration.field("privileged", debug(request.privileged()));
        configuration.field("readonly", debug(request.readonly_rootfs()));
        configuration.field("shm", debug(request.shm_size()));
        configuration.field("cgroupns", debug(request.cgroupns_mode()));
        configuration.field("userns", request.userns_mode().unwrap_or_default());
        configuration.field("cap_add", debug(request.cap_add()));
        configuration.field("cap_drop", debug(request.cap_drop()));
        configuration.field("security", debug(request.security_opts()));
        configuration.field("health", debug(request.health_check()));
        configuration.field("stdin", debug(request.open_stdin()));
        configuration.field("extra", self.discriminator.as_deref().unwrap_or_default());
        configuration.0
    }
}

fn debug(value: impl std::fmt::Debug) -> String {
    format!("{value:?}")
}

/// An injective encoding of the fields identifying a container.
///
/// Every value is length-prefixed, so no value — a password holding a `;`, a
/// command holding a `=` — can imitate a field or list boundary, and two
/// different field sets can never produce the same string. Only its
/// fingerprint is ever stored, so it is built for uniqueness, not for reading.
#[derive(Default)]
struct Configuration(String);

impl Configuration {
    fn field(&mut self, key: &str, value: impl AsRef<str>) {
        let value = value.as_ref();
        self.0.push_str(&format!("{key}={}:{value};", value.len()));
    }

    fn list<S: AsRef<str>>(&mut self, key: &str, values: impl IntoIterator<Item = S>) {
        let mut encoded = Configuration::default();
        for value in values {
            encoded.field("", value);
        }
        self.field(key, encoded.0);
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
                    panic!(
                        "cannot resolve the mapped {service} port {container_port} — is it \
                         exposed by the image (`with_exposed_port`/`Image::expose_ports`)?: \
                         {error}"
                    )
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
