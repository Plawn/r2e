//! Application builder: two-phase assembly of an R2E app.
//!
//! - [`nostate`]: the pre-state phase (`AppBuilder<NoState>`) — bean/producer
//!   registration, config loading, pre-state plugins, `build_state`.
//! - [`typed`]: the typed phase (`AppBuilder<T>`) — controllers, plugins,
//!   layers, hooks, `build()` / `prepare()` / `serve()`.
//! - [`prepared`]: [`PreparedApp`] + the serving lifecycle (`run()`).
//! - [`task_registry`]: [`TaskRegistryHandle`] shared by scheduler/gRPC/plugins.

mod app;
mod bootable;
mod nostate;
mod prepared;
mod registration;
mod task_registry;
mod typed;

pub use app::{launch, App};
pub use bootable::BootableApp;
pub use prepared::PreparedApp;
pub use registration::{RegisterController, RegisterControllers, RegisterModule};
pub use task_registry::{ScheduledTaskMarker, TaskRegistryHandle};

use crate::beans::{AsyncBean, Bean, BeanRegistry, Producer, Registrable};
use crate::controller::Controller;
use crate::module::{
    BeanList, ControllerDepsList, ExportsProvided, FeatureModule, ModuleDepsSatisfied, ModuleList,
    ModuleScope, RequiredPluginsInstalled,
};
use crate::lifecycle::{DrainHook, ShutdownHook, StartupHook, StopHandle};
use crate::meta::MetaRegistry;
use crate::service::ServiceComponent;
use crate::plugin::{DeferredAction, DeferredContext, Plugin, RawPreStatePlugin};
use crate::type_list::{AllSatisfied, BuildHList, TAppend, TCons, TNil};
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::info;

/// Builder returned by the NoState registration methods
/// ([`register`](AppBuilder::register), [`with_default_bean`](AppBuilder::with_default_bean), …):
/// `Provided` is pushed onto the provision list `P` and `Deps` is appended to
/// the requirement list `R`.
pub type Registered<Provided, Deps, P, R, Mods> =
    AppBuilder<NoState, TCons<Provided, P>, <R as TAppend<Deps>>::Output, Mods>;

/// Builder returned by [`load_config`](AppBuilder::load_config): pushes the
/// typed config `C`, the raw [`R2eConfig`](crate::config::R2eConfig), and
/// `C`'s nested section types (`C::Children`) onto the provision list.
pub type WithLoadedConfig<C, P, R, Mods> = AppBuilder<
    NoState,
    TCons<
        C,
        TCons<
            crate::config::R2eConfig,
            <<C as crate::config::LoadableConfig>::Children as TAppend<P>>::Output,
        >,
    >,
    R,
    Mods,
>;

/// Builder returned by [`plugin`](AppBuilder::plugin): the plugin's
/// `Provisions` join `P`, and **both** its call-site `Required` (`Deps`) and
/// its post-state `LateRequired` (`LateDeps`) join `R`. Only `Required` is
/// checked at the call site; `LateRequired` is verified against the final
/// provision list at `build_state()`.
pub type WithPluginInstalled<Pl, P, R, Mods> = AppBuilder<
    NoState,
    <P as TAppend<<Pl as RawPreStatePlugin>::Provisions>>::Output,
    <R as TAppend<<Pl as RawPreStatePlugin>::AllRequired>>::Output,
    Mods,
>;

/// Builder returned by
/// [`register_module`](registration::RegisterModule::register_module): the
/// module's `Exports` join the provision list `P`, its `Imports` join the
/// requirement list `R`, and the module is queued on `Mods` so
/// `build_state()` registers its controllers.
pub type ModuleRegistered<M, P, R, Mods> = AppBuilder<
    NoState,
    <<M as FeatureModule>::Exports as TAppend<P>>::Output,
    <R as TAppend<<M as FeatureModule>::Imports>>::Output,
    TCons<M, Mods>,
>;

type ConsumerReg<T> =
    Box<dyn FnOnce(T) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send>;

/// A queued controller-core `#[post_construct]` future, awaited at startup
/// before consumer registrations. State-free (the future already captures the
/// core `Arc`), so — unlike [`ConsumerReg`] — it carries no `T`.
type PostConstructReg = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send>,
>;

type LayerFn = Box<dyn FnOnce(crate::http::Router) -> crate::http::Router + Send>;

/// A meta consumer that drains typed metadata from the registry and returns
/// a router fragment to be merged into the application.
type MetaConsumer<T> = Box<dyn FnOnce(&MetaRegistry) -> crate::http::Router<T> + Send>;

/// A serve hook, called once when the server starts. Receives a
/// [`ServeContext`] tying the hook into the app's shutdown sequence.
type ServeHook = Box<dyn FnOnce(ServeContext) + Send>;

/// Context handed to serve hooks ([`DeferredContext::on_serve`]) when the
/// server starts.
///
/// Ties serve-time subsystems into the app's lifecycle:
/// - [`task_registry`](Self::task_registry) — the shared task registry; each
///   hook drains the tasks it owns (`take_of::<Tag>()` for tagged tasks, or
///   `take_all()` for single-consumer subsystems).
/// - [`shutdown_token`](Self::shutdown_token) — cancelled when graceful
///   shutdown begins (after drain hooks), while in-flight HTTP requests are
///   still finishing. Use it to stop accepting new work.
/// - [`track`](Self::track) — register a spawned task whose completion is
///   awaited after the HTTP drain, before user shutdown hooks (bounded by
///   [`AppBuilder::shutdown_grace_period`]). Track any server-like task that
///   drains on the shutdown token so the process doesn't exit mid-drain.
pub struct ServeContext {
    tasks: TaskRegistryHandle,
    shutdown: CancellationToken,
    handles: ServiceHandles,
}

impl ServeContext {
    /// The shared task registry (scheduled tasks, tagged subsystem tasks).
    pub fn task_registry(&self) -> TaskRegistryHandle {
        self.tasks.clone()
    }

    /// Token cancelled when graceful shutdown begins.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    /// Track a task handle to be awaited after the HTTP drain completes.
    pub fn track(&self, handle: crate::rt::JobHandle<()>) {
        self.handles.push(handle);
    }
}

/// Shared collection of JobHandles awaited after the HTTP drain: services
/// spawned via [`AppBuilder::spawn_service`] and serve-hook tasks registered
/// through [`ServeContext::track`]. Shutdown awaits their completion
/// with a grace deadline before returning.
#[derive(Clone, Default)]
struct ServiceHandles(Arc<Mutex<Vec<crate::rt::JobHandle<()>>>>);

impl ServiceHandles {
    fn push(&self, handle: crate::rt::JobHandle<()>) {
        self.0.lock().unwrap().push(handle);
    }

    fn drain(&self) -> Vec<crate::rt::JobHandle<()>> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

/// Resolve the active profile: forced (`with_profile`) > `R2E_PROFILE` env >
/// `r2e.profile` config > `"default"`.
fn resolve_profile(forced: Option<&str>, config: &crate::config::R2eConfig) -> String {
    forced
        .map(str::to_string)
        .or_else(|| std::env::var("R2E_PROFILE").ok())
        .or_else(|| config.try_get::<String>("r2e.profile"))
        .unwrap_or_else(|| "default".to_string())
}

/// Marker type: application state has not been set yet.
///
/// `AppBuilder<NoState>` is the initial phase returned by [`AppBuilder::new()`].
/// Call [`.with_state()`](AppBuilder::with_state) or [`.build_state()`](AppBuilder::build_state)
/// to transition to `AppBuilder<T>`.
#[derive(Clone)]
pub struct NoState;

/// Shared configuration that is independent of the application state type.
struct BuilderConfig {
    config: Option<crate::config::R2eConfig>,
    custom_layers: Vec<LayerFn>,
    /// Transport-level router transforms, applied OUTERMOST — after
    /// `custom_layers` and `catch_panic_layer`. The wrapped service sees
    /// every request before any HTTP middleware; the inner HTTP router keeps
    /// its full middleware stack. Used by transport multiplexers (e.g. gRPC
    /// content-type routing) so non-HTTP traffic never crosses HTTP-shaped
    /// layers.
    router_wraps: Vec<LayerFn>,
    bean_registry: BeanRegistry,
    /// Deferred actions to be executed after state resolution.
    deferred_actions: Vec<DeferredAction>,
    /// Plugin data storage (type-erased, keyed by TypeId).
    plugin_data: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    /// Name of the last plugin that should be installed last (for ordering validation).
    last_plugin_name: Option<&'static str>,
    /// Whether to install the pre-routing trailing-slash normalization rewrite.
    normalize_path: bool,
    /// Whether the DevReload plugin has been applied (prevents double-install).
    dev_reload_applied: bool,
    /// Maximum time allowed for shutdown hooks to complete before force-exiting.
    /// `None` means wait indefinitely (default).
    shutdown_grace_period: Option<Duration>,
    /// Active profile name, resolved from the forced profile
    /// ([`AppBuilder::with_profile`]), `R2E_PROFILE` env var, `r2e.profile`
    /// config key, or `"default"`.
    active_profile: String,
    /// Profile forced via [`AppBuilder::with_profile`]; wins over env/config
    /// detection. Set by test harnesses (no process-global env mutation).
    forced_profile: Option<String>,
    /// Base config file set via [`AppBuilder::with_config_file`]; used by
    /// `load_config` instead of the default `application.yaml`.
    config_file: Option<std::path::PathBuf>,
    /// Config keys stashed via [`AppBuilder::override_config_value`] before
    /// config is loaded; applied on top of whatever `load_config` produces
    /// (always drained *after* the base config, so they win).
    config_overrides: Vec<(String, crate::config::ConfigValue)>,
    /// Pre-loaded config stashed via [`AppBuilder::override_config`]; consumed
    /// by `load_config` in place of the disk read. Test harnesses and the
    /// dev-reload loop set it. Left `Some` at `build_state` (never consumed by
    /// a `load_config` call) is a panic — the config would be silently ignored.
    preloaded_config: Option<crate::config::R2eConfig>,
    /// Stop handle wired via [`AppBuilder::with_stop_handle`]; `prepare()`
    /// creates one lazily when absent.
    stop_handle: Option<StopHandle>,
    /// Pre-destroy disposers, drained from the resolved [`BeanContext`] at
    /// `build_state()` and folded into the async shutdown phase by
    /// [`AppBuilder::from_pre`].
    bean_disposers: Vec<crate::plugin::AsyncShutdownHook>,
}

/// Builder for assembling a R2E application.
///
/// Collects state, controller routes, and Tower layers, then produces an
/// `axum::Router` (or starts serving directly) with everything wired together.
///
/// # Two-phase builder
///
/// The builder starts in the `NoState` phase (`AppBuilder<NoState>`), where
/// you can call [`provide()`](Self::provide), [`register()`](Self::register),
/// and state-independent configuration methods. Transition to a typed phase
/// via:
///
/// - [`.with_state(state)`](AppBuilder::<NoState>::with_state) — provide a pre-built state directly.
/// - [`.build_state()`](AppBuilder::<NoState>::build_state) — resolve the bean graph and build state.
///
/// Once in the typed phase (`AppBuilder<T>`), you can register controllers,
/// install plugins via [`.with()`](Self::with), add hooks, and call `.build()`
/// or `.serve()`.
pub struct AppBuilder<T: Clone + Send + Sync + 'static = NoState, P = TNil, R = TNil, Mods = TNil> {
    shared: BuilderConfig,
    state: T,
    /// The resolved bean graph, retained through the typed phase so controller
    /// cores (and background services) can be constructed by type via
    /// `ctx.get::<T>()`. An empty placeholder before `build_state()` and on the
    /// `with_state` path.
    bean_context: Arc<crate::beans::BeanContext>,
    routes: Vec<crate::http::Router<T>>,
    startup_hooks: Vec<StartupHook<T>>,
    shutdown_hooks: Vec<ShutdownHook<T>>,
    drain_hooks: Vec<DrainHook<T>>,
    meta_registry: MetaRegistry,
    meta_consumers: Vec<MetaConsumer<T>>,
    consumer_registrations: Vec<ConsumerReg<T>>,
    /// Controller-core `#[post_construct]` futures, awaited at startup before
    /// `consumer_registrations`.
    post_construct_registrations: Vec<PostConstructReg>,
    /// Serve hooks from plugins (called when server starts).
    /// Tasks already capture their state, so only the token is needed.
    serve_hooks: Vec<ServeHook>,
    /// Shutdown hooks from plugins (sync).
    plugin_shutdown_hooks: Vec<Box<dyn FnOnce() + Send>>,
    /// Shutdown hooks from plugins (async, awaited during shutdown).
    plugin_async_shutdown_hooks: Vec<crate::plugin::AsyncShutdownHook>,
    _provided: PhantomData<P>,
    _required: PhantomData<R>,
    /// Pending feature modules whose controllers `build_state()` registers.
    _modules: PhantomData<Mods>,
}

// ── Conditional assembly (any phase) ────────────────────────────────────────

impl<T: Clone + Send + Sync + 'static, P, R, Mods> AppBuilder<T, P, R, Mods> {
    /// Conditionally apply a builder transformation.
    ///
    /// `f` must return the **same** builder type, so it may call `Self -> Self`
    /// methods (custom layers, plugins, config toggles) but **not** type-changing
    /// methods like `register`: a runtime flag cannot change the compile-time
    /// provision list `P`. For conditional *bean* presence, use a
    /// `#[producer] -> Option<T>` — the slot is always in `P` and the producer
    /// decides `Some`/`None` internally.
    ///
    /// ```ignore
    /// AppBuilder::new()
    ///     .when(cfg!(debug_assertions), |b| b.with(DevReload))
    /// ```
    pub fn when(self, cond: bool, f: impl FnOnce(Self) -> Self) -> Self {
        if cond {
            f(self)
        } else {
            self
        }
    }

    /// Wire a user-created [`StopHandle`] into the server lifecycle.
    ///
    /// Calling [`StopHandle::stop`] on (a clone of) the handle triggers the
    /// same graceful shutdown as an OS signal.
    ///
    /// Usually unnecessary: a `StopHandle` bean (`.provide(stop.clone())`,
    /// e.g. for an admin endpoint) is picked up automatically at
    /// [`prepare()`](AppBuilder::prepare), and without one
    /// [`PreparedApp::stop_handle`] hands back a fresh wired handle. Use this
    /// only to wire a handle that is neither a bean nor taken from the
    /// prepared app (it takes precedence over a bean).
    pub fn with_stop_handle(mut self, handle: StopHandle) -> Self {
        self.shared.stop_handle = Some(handle);
        self
    }
}

impl Default for AppBuilder<NoState, TNil, TNil, TNil> {
    fn default() -> Self {
        Self::new()
    }
}
