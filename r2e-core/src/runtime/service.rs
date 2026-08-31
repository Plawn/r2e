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

    /// Run until the shutdown token is cancelled.
    fn start(self, shutdown: CancelToken) -> impl Future<Output = ()> + Send;
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
