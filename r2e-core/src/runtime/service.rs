use crate::rt::CancelToken;
use std::future::Future;

/// A background service that participates in DI but doesn't handle HTTP.
///
/// Implement this trait for long-running background components (queue
/// consumers, gRPC servers, metrics exporters, etc.) that need access
/// to application beans but are not HTTP handlers. Construction pulls beans
/// from the resolved graph by type — the same model as controller cores.
///
/// # Example
///
/// ```ignore
/// struct MetricsExporter {
///     pool: SqlitePool,
/// }
///
/// impl ServiceComponent for MetricsExporter {
///     type Deps = TCons<SqlitePool, TNil>;
///
///     fn from_context(ctx: &BeanContext) -> Self {
///         Self { pool: ctx.get::<SqlitePool>() }
///     }
///
///     async fn start(self, shutdown: CancelToken) {
///         loop {
///             rt::select! {
///                 _ = shutdown.cancelled() => break,
///                 _ = rt::sleep(Duration::from_secs(60)) => {
///                     // export metrics...
///                 }
///             }
///         }
///     }
/// }
///
/// // Register in builder:
/// AppBuilder::new()
///     .provide(pool)
///     .build_state().await
///     .spawn_service::<MetricsExporter>()
///     .serve("0.0.0.0:3000").await
/// ```
pub trait ServiceComponent: Sized + Send + 'static {
    /// Type-level list ([`TCons`](crate::type_list::TCons) /
    /// [`TNil`](crate::type_list::TNil)) of the bean types
    /// [`from_context`](Self::from_context) pulls — including `R2eConfig` when
    /// the service has `#[config]` fields and `LiveConfigRegistry` when it has
    /// `#[live_config]` ones.
    ///
    /// Checked against the application state at
    /// [`spawn_service`](crate::builder::SpawnService::spawn_service) via
    /// [`AllSatisfied`](crate::type_list::AllSatisfied), so a service reading a
    /// bean that is absent from the graph is a **compile** error at the
    /// registration call site instead of a `ctx.get()` panic at startup.
    /// `#[derive(BackgroundService)]` emits it; hand-written impls that build
    /// from an already-provided value use `TNil`.
    ///
    /// The `#[producer(start)]` path has no state type of its own to check
    /// against, so `#[producer]` folds `<Output as ServiceComponent>::Deps`
    /// into the producer's own `Producer`/`Registrable` `Deps`: the service's
    /// beans are demanded at the `.register::<TheProducer>()` call site,
    /// exactly like the producer function's parameters. A produced service
    /// reading an absent bean is therefore also a compile error, not a
    /// `from_context` panic when the task starts.
    type Deps;

    /// The config keys [`from_context`](Self::from_context) reads, as
    /// `(key, type name, kind)` — the
    /// [`Bean::config_keys`](crate::beans::Bean::config_keys) counterpart for
    /// background services, emitted by `#[derive(BackgroundService)]`.
    ///
    /// `Required` entries are presence-validated where the service is
    /// registered — at [`spawn_service`](crate::builder::SpawnService::spawn_service)
    /// (aggregated panic naming the service) and, for `#[producer(start)]`
    /// services, during graph resolution alongside the bean keys. Default:
    /// empty.
    ///
    /// `Section` entries appear here for completeness but cannot be validated
    /// from a type *name* — see [`config_sections`](Self::config_sections).
    fn config_keys() -> Vec<(&'static str, &'static str, crate::config::ConfigKeyKind)> {
        Vec::new()
    }

    /// The `#[config_section]` prefixes [`from_context`](Self::from_context)
    /// builds, as type-aware [`SectionValidator`](crate::config::SectionValidator)s.
    ///
    /// A background service constructs when its task starts, long after
    /// startup validation, so a missing section key would surface as a panic
    /// inside `from_context`. Declaring the sections here lets the same
    /// registration points that check [`config_keys`](Self::config_keys) run
    /// the full `validate_section::<Ty>` walk instead. Emitted by
    /// `#[derive(BackgroundService)]`. Default: empty.
    fn config_sections() -> Vec<crate::config::SectionValidator> {
        Vec::new()
    }

    /// Construct from the resolved bean graph.
    fn from_context(ctx: &crate::beans::BeanContext) -> Self;

    /// Opt-in gate: return `false` to keep this service from running.
    ///
    /// Evaluated **once**, on the constructed instance, at the moment the
    /// service task would call [`start`](Self::start) — on every spawn path
    /// ([`spawn_service`](crate::builder::SpawnService::spawn_service),
    /// `#[producer(start)]`, and `#[bean]`-declared services). Default: `true`.
    ///
    /// What it deliberately does **not** skip: registration, dependency
    /// resolution, [`from_context`](Self::from_context), and the
    /// [`config_keys`](Self::config_keys) /
    /// [`config_sections`](Self::config_sections) validation. A disabled
    /// service is still a fully declared, fully validated part of the
    /// application — only `run()` is skipped, and the framework logs an
    /// `info!` naming the service and (when the derive could work it out) the
    /// gate that turned it off. Turning a service off must never turn its
    /// configuration errors off with it.
    ///
    /// `#[derive(BackgroundService)]` emits this from
    /// `#[service(enabled = "…")]`, naming either a `&self` method returning
    /// `bool` or a `bool` field of the struct — typically a
    /// `#[config("services.x.enabled")] enabled: bool`.
    ///
    /// Composes with the global [`SERVICES_ENABLED_KEY`] switch: the service
    /// runs only when the global gate **and** this one both say yes.
    fn enabled(&self) -> bool {
        true
    }

    /// Human-readable label for whatever [`enabled`](Self::enabled) reads —
    /// the config key when the derive can see one, otherwise the field or
    /// method name. Logged when the gate turns the service off, so the reader
    /// learns *which* switch to flip. Default: `None`.
    fn enabled_gate() -> Option<&'static str> {
        None
    }

    /// When, in the shutdown sequence, this service's token is cancelled.
    /// Default: [`StopPhase::Early`] — before the HTTP drain, together with
    /// every other background task.
    ///
    /// Return [`StopPhase::AfterDrain`] for a service that **consumes work the
    /// request path produces** (a write-behind sink, a batching writer, an
    /// outbox flusher fed by handlers through a channel): it is then cancelled
    /// only once in-flight requests have finished, so nothing a request
    /// enqueued during the drain is lost. `#[derive(BackgroundService)]` emits
    /// this from `#[service(stop = "after_drain")]`.
    fn stop_phase() -> StopPhase {
        StopPhase::Early
    }

    /// Position among the [`AfterDrain`](StopPhase::AfterDrain) services.
    /// They are stopped **one at a time**, lowest order first (registration
    /// order among equals): each is cancelled and joined — bounded by
    /// `shutdown_grace_period` — before the next one is told to stop, so a
    /// producer can be given a lower order than the sink it feeds. Ignored
    /// for [`Early`](StopPhase::Early) services. Default: `0`.
    fn stop_order() -> i32 {
        0
    }

    /// Run until the shutdown token is cancelled.
    fn start(self, shutdown: CancelToken) -> impl Future<Output = ()> + Send;
}

/// When a [`ServiceComponent`]'s token is cancelled during graceful shutdown.
///
/// The shutdown sequence is `on_drain` hooks → plugin shutdown hooks (which
/// cancel the **`Early`** services) → HTTP drain → tracked-handle join →
/// **`AfterDrain`** services, one at a time → post-drain plugin hooks →
/// `on_stop`. See `docs/features/22-serve-lifecycle.md`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum StopPhase {
    /// Cancelled before the listener stops accepting, with every other
    /// background task (the default). Right for pollers, exporters and
    /// anything that *produces* work.
    #[default]
    Early,
    /// Cancelled after the HTTP drain and the tracked-handle join, i.e. after
    /// the last in-flight request has returned. Right for anything that
    /// *consumes* work requests hand it. Each `AfterDrain` service is joined
    /// (under `shutdown_grace_period`) before the next is cancelled, in
    /// [`stop_order`](ServiceComponent::stop_order) order, so this phase can
    /// cost up to one grace period per service.
    AfterDrain,
}

/// What the builder reads off a [`ServiceComponent`] to place it in the
/// shutdown sequence: its [`StopPhase`] and its
/// [`stop_order`](ServiceComponent::stop_order).
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StopSpec {
    pub phase: StopPhase,
    pub order: i32,
}

impl StopSpec {
    /// The spec a service type declares.
    pub fn of<C: ServiceComponent>() -> Self {
        Self {
            phase: C::stop_phase(),
            order: C::stop_order(),
        }
    }
}

/// Config key of the **global** background-service gate.
///
/// `services.enabled: false` keeps every background service from running —
/// the profile-level switch a `application-test.yaml` flips so a test boot
/// does not start pollers, exporters or queue consumers. It composes with the
/// per-service [`ServiceComponent::enabled`] gate: a service runs only when
/// *both* say yes.
///
/// Like the per-service gate it skips **only** `start()`. Registration,
/// dependency resolution, `from_context` and config validation all still run,
/// so a test with services off still fails on a broken service configuration.
///
/// Absent or non-boolean → services are enabled (default `true`).
pub const SERVICES_ENABLED_KEY: &str = "services.enabled";

/// Read the global gate from the application config.
///
/// `None` (no config loaded) → enabled: an app that never called
/// `load_config` cannot have opted out.
pub fn services_enabled(config: Option<&crate::config::R2eConfig>) -> bool {
    config
        .and_then(|c| c.try_get::<bool>(SERVICES_ENABLED_KEY))
        .unwrap_or(true)
}

static GLOBAL_GATE_LOGGED: std::sync::Once = std::sync::Once::new();

/// Log the "no background service will run" line — once per process, however
/// many services are skipped, so the boot log carries the cause without one
/// line per service.
pub(crate) fn log_services_globally_disabled() {
    GLOBAL_GATE_LOGGED.call_once(|| {
        tracing::info!(
            gate = SERVICES_ENABLED_KEY,
            "background services globally disabled — every service is still registered and \
             config-validated, but no run() will be called"
        );
    });
}

/// Log the framework-level "this service will not run" line, shared by every
/// spawn path so the message and its fields are identical wherever the gate
/// fires.
///
/// `info!`, not `warn!`: a service disabled by its own declared gate is a
/// configured outcome, not a problem. It is still logged unconditionally —
/// "why is nothing happening?" must be answerable from the boot log.
pub(crate) fn log_service_disabled(service: &'static str, gate: Option<&'static str>) {
    tracing::info!(
        service,
        gate = gate.unwrap_or("ServiceComponent::enabled"),
        "background service disabled by its `enabled` gate — registered and \
         config-validated, but run() will not be called"
    );
}
