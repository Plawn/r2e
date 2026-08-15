use std::any::Any;
use std::collections::{BTreeMap, HashMap};
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
/// `ContainerRequest` is not `Clone` and a contended start is retried — so the
/// closure must build the same request every time (see [`new`](Self::new)).
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
    ///
    /// `request` must be **deterministic**: it is called again for the identity
    /// and on every start attempt, and a container is only ever as described as
    /// the request the identity was derived from. The shared path re-derives
    /// each attempt and panics on a mismatch rather than start a container its
    /// name does not describe.
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
    ///
    /// # Panics
    ///
    /// Panics on a blank discriminator. It is the caller's description of what
    /// the identity cannot read, and a blank one describes nothing while still
    /// satisfying the checks that ask for one.
    pub fn with_discriminator(mut self, discriminator: impl Into<String>) -> Self {
        let discriminator = discriminator.into();
        assert!(
            !discriminator.trim().is_empty(),
            "the {} dev service was given a blank discriminator — it is what tells two \
             containers apart when the request cannot, so it has to say something",
            self.service
        );
        self.discriminator = Some(discriminator);
        self
    }

    /// The canonical description of everything that shapes the container.
    ///
    /// Fingerprinted into the container name and the
    /// `dev.r2e.devservices.config` label: same string ⇒ same shared container.
    /// It covers the image type and reference, the declared ports, and the
    /// request fields that shape the container Docker creates — env vars,
    /// labels, command, entrypoint, mounts, copied files, port mappings, device
    /// requests, hosts, network, platform, user, namespaces, capabilities,
    /// security options, health check — so two requests differing in any of
    /// them get two containers.
    ///
    /// Each field is folded the way Docker resolves it, because merging two
    /// different requests onto one container is a bug while splitting two
    /// identical ones is only waste:
    ///
    /// - *Resolved by key* — env vars, labels, hosts, port mappings: folded
    ///   into a map first, so it is the **effective** value that counts (an
    ///   overridden env var, the last host port bound to a container port).
    /// - *Set-like* — exposed ports, mounts, capabilities: sorted, so
    ///   declaration order alone never splits a container.
    /// - *Ordered* — command, copied files, device requests, security options:
    ///   digested in order, because Docker applies them in order and a later
    ///   one can override an earlier one.
    ///
    /// What it cannot see, all of it grounds for
    /// [`with_discriminator`](Self::with_discriminator):
    ///
    /// - ulimits, which testcontainers keeps private;
    /// - what a host-config modifier *does* — only whether one is set is
    ///   readable, a closure's effect has no representation to fingerprint. The
    ///   shared path therefore refuses a modifier without a discriminator
    ///   rather than merge two containers it cannot tell apart;
    /// - the *contents* of a file copied by path (only the path is visible from
    ///   here — a fixture edited in place keeps its identity);
    /// - anything applied *after* start: seeded data, and the exec hooks an
    ///   `Image` runs itself (`exec_before_ready`, `exec_after_start`), which
    ///   two same-typed images can drive from internal state invisible here.
    ///
    /// Deliberately excluded: readiness conditions and the startup timeout
    /// change how long we wait, not what runs. Host-port exposures are not
    /// encoded either — testcontainers rejects them outright for reusable
    /// containers, and the shared path always asks for reuse.
    ///
    /// The request factory is invoked again on every start attempt, and each
    /// result is re-derived and compared against this one: a factory that does
    /// not build the same request every time is rejected rather than allowed to
    /// start a container its identity does not describe. That comparison only
    /// sees what this string sees, so a factory varying an opaque field — an
    /// ulimit, what a modifier closure does — still slips through; the
    /// determinism requirement is the contract, the check is a backstop.
    #[doc(hidden)]
    pub fn configuration(&self) -> String {
        self.configuration_of(&(self.request)())
    }

    /// Refuse to share a container whose shape the identity cannot describe.
    ///
    /// A host-config modifier rewrites the Docker configuration from a closure:
    /// two specs capping memory at 64 MiB and at 128 MiB are indistinguishable
    /// from here, and would land on one container — the exact failure the
    /// derived identity exists to prevent. Only the *presence* of a modifier is
    /// readable, so the discriminator is where the caller says what it does.
    /// [`start`](DevService::start) is unaffected: nothing is shared there.
    ///
    /// Checked against the request about to start, not a fresh one: a factory
    /// returning a bare request first and a modified one later would otherwise
    /// walk past a check made on the first.
    fn ensure_shareable(&self, request: &ContainerRequest<I>) {
        assert!(
            self.discriminator.is_some() || request.host_config_modifier().is_none(),
            "the {} dev service sets a host-config modifier, and the sharing identity cannot \
             read what it does — two different modifiers would share one container. Describe \
             it with .with_discriminator(\"...\"), or use DevService::start for an isolated \
             container.",
            self.service
        );
    }

    fn configuration_of(&self, request: &ContainerRequest<I>) -> String {
        let mut ports = self.ports.clone();
        ports.sort_unstable();
        // `env_vars()` yields the image's variables first and the request's
        // second, and Docker keeps the last value for a name. Folding in that
        // order therefore records the *effective* environment: sorting the
        // flattened `name=value` pairs instead would give `MODE=a` overridden
        // by `MODE=b` the same identity as the reverse, which run differently.
        // The map also settles the order — some `Image` impls hold their
        // variables in a `HashMap` (the Postgres module does), so the same spec
        // would otherwise fingerprint differently in two test binaries.
        let env: BTreeMap<String, String> = request
            .env_vars()
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect();

        let mut configuration = Configuration::default();
        configuration.field("type", std::any::type_name::<I>());
        configuration.field("image", request.descriptor());
        configuration.list("port", ports.iter().map(u16::to_string));
        // Set-like on Docker's side: sorted, so two requests that declare the
        // same ports or capabilities in a different order share a container.
        configuration.list("expose", sorted(request.expose_ports().iter().map(debug)));
        // Port mappings are keyed by container port on the way to Docker
        // (`port_bindings` is a map), so a repeated container port keeps its
        // last host port. Folding the same way records the binding that will
        // actually apply; sorting the mappings whole would give
        // `[8080→80, 9090→80]` and its reverse — which bind different host
        // ports — the same identity.
        configuration.pairs(
            "map",
            &request
                .ports()
                .into_iter()
                .flatten()
                .map(|mapping| {
                    (
                        debug(mapping.container_port()),
                        mapping.host_port().to_string(),
                    )
                })
                .collect(),
        );
        configuration.field("entrypoint", optional(request.entrypoint()));
        // Ordered, unlike the above: argv and copy order both change the result.
        configuration.list("cmd", request.cmd());
        configuration.pairs("env", &env);
        configuration.pairs("label", request.labels());
        configuration.pairs(
            "host",
            &request
                .hosts()
                .map(|(name, host)| (name.into_owned(), debug(host)))
                .collect(),
        );
        configuration.list("mount", sorted(request.mounts().map(debug)));
        configuration.list("copy", request.copy_to_sources().map(digest));
        configuration.field("network", optional(request.network().as_deref()));
        configuration.field("hostname", optional(request.hostname()));
        configuration.field("platform", optional(request.platform().as_deref()));
        configuration.field("workdir", optional(request.working_dir()));
        configuration.field("user", optional(request.user()));
        // Only whether one is set: the closure's effect is unreadable, which is
        // why the shared path refuses a modifier without a discriminator.
        configuration.field("modifier", debug(request.host_config_modifier().is_some()));
        configuration.field("privileged", debug(request.privileged()));
        configuration.field("readonly", debug(request.readonly_rootfs()));
        configuration.field("shm", debug(request.shm_size()));
        configuration.field("cgroupns", debug(request.cgroupns_mode()));
        configuration.field("userns", optional(request.userns_mode()));
        configuration.list("cap_add", sorted(capabilities(request.cap_add())));
        configuration.list("cap_drop", sorted(capabilities(request.cap_drop())));
        // Ordered as well: Docker parses security options in sequence and a
        // later one overwrites an earlier one with the same name, so
        // `no-new-privileges=true` then `=false` is not the reverse pair.
        configuration.list("security", capabilities(request.security_opts()));
        // Ordered too, despite looking set-like: Docker keeps the vector and
        // applies each request in turn to the same OCI spec — the NVIDIA
        // handler appends to `NVIDIA_VISIBLE_DEVICES` per request — so two
        // orders can produce two different containers.
        configuration.list(
            "device",
            request.device_requests().into_iter().flatten().map(digest),
        );
        configuration.field("health", debug(request.health_check()));
        configuration.field("stdin", debug(request.open_stdin()));
        configuration.field("extra", optional(self.discriminator.as_deref()));
        configuration.0
    }
}

fn debug(value: impl std::fmt::Debug) -> String {
    format!("{value:?}")
}

fn sorted(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut values: Vec<String> = values.into_iter().collect();
    values.sort();
    values
}

fn capabilities(values: Option<&Vec<String>>) -> impl Iterator<Item = String> + '_ {
    values.into_iter().flatten().cloned()
}

/// An optional string, encoded so that `None` and `Some("")` stay apart.
///
/// Docker normalizes most empty values back to unset, so telling them apart
/// usually splits two containers that would have run the same — waste, and
/// waste is the safe direction here. Collapsing them with `unwrap_or_default`
/// would instead put a request that sets a field to nothing and one that never
/// set it on the same container, and break the encoding's injectivity.
fn optional(value: Option<&str>) -> String {
    value.map(|value| format!("+{value}")).unwrap_or_default()
}

/// The `Debug` form of a value, folded into a fixed-size digest.
///
/// For values that can be arbitrarily large — a `CopyToContainer` carrying an
/// in-memory asset prints every byte as a decimal number — materializing that
/// form would cost several times the asset itself, twice over once the nested
/// encoding copies it again. Folding to 64 bits admits collisions in principle;
/// the identity is fingerprinted to the same width anyway.
fn digest(value: impl std::fmt::Debug) -> String {
    use std::fmt::Write;

    struct Digest(u64);

    impl Write for Digest {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            self.0 = common::fnv1a(self.0, text.as_bytes());
            Ok(())
        }
    }

    let mut digest = Digest(common::FNV_OFFSET);
    let _ = write!(digest, "{value:?}");
    format!("{:016x}", digest.0)
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

    /// Both halves of an entry are length-prefixed on their own, so a name
    /// holding an `=` cannot be read as a shorter name and a longer value.
    fn pairs(&mut self, key: &str, entries: &BTreeMap<String, String>) {
        let mut encoded = Configuration::default();
        for (name, value) in entries {
            encoded.field("", name);
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
        // `Never` overrides whatever the spec asked for: testcontainers skips
        // removal on drop for the reusing directives, which would leave this
        // container behind — and the handle is the only thing scoping it.
        // Labelled from the request that starts, not from a second one the
        // factory might build differently — nothing is shared here, but the
        // label should still describe this container.
        let built = (spec.request)();
        let configuration = spec.configuration_of(&built);
        let request = common::label_isolated(built, &spec.service, &configuration)
            .with_reuse(ReuseDirective::Never);
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
    /// Panics if Docker is unavailable, if the container fails to start, or if
    /// the spec sets a host-config modifier without a
    /// [`discriminator`](DevServiceSpec::with_discriminator) — see
    /// [`configuration`](DevServiceSpec::configuration).
    pub async fn shared<I: Image + 'static>(spec: DevServiceSpec<I>) -> &'static Self {
        // One cell per (service, configuration), leaked to hand out `&'static`
        // for the process's lifetime — the container lives as long as the cell
        // that owns it. A single registry serves every service because the
        // container type is erased.
        static SHARED: OnceLock<Mutex<HashMap<String, &'static OnceCell<DevService>>>> =
            OnceLock::new();

        let configuration = spec.configuration();
        spec.ensure_shareable(&(spec.request)());

        let identity = common::SharedIdentity::new(&spec.service, &configuration);
        let cell = {
            let mut cells = SHARED
                .get_or_init(Mutex::default)
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            *cells
                .entry(identity.name().to_string())
                .or_insert_with(|| Box::leak(Box::new(OnceCell::const_new())))
        };
        cell.get_or_init(|| Self::start_shared(spec, identity, configuration))
            .await
    }

    async fn start_shared<I: Image + 'static>(
        spec: DevServiceSpec<I>,
        identity: common::SharedIdentity,
        configuration: String,
    ) -> Self {
        ryuk::ensure_lease().await;
        common::cleanup(&identity).await;

        let container = common::start_with_retry(&spec.service, || {
            // The factory runs again per attempt, so the request that starts is
            // not the one the identity was taken from. Re-deriving here is what
            // makes the identity a promise about the container that actually
            // runs: a factory alternating between two requests would otherwise
            // start the second under the first one's name.
            let request = (spec.request)();
            spec.ensure_shareable(&request);
            assert_eq!(
                spec.configuration_of(&request),
                configuration,
                "the {} dev service's request factory built a different container on a later \
                 call — it is invoked again on every start attempt and must be deterministic",
                spec.service
            );
            identity.label(
                request
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
