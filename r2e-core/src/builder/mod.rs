//! Application builder: two-phase assembly of an R2E app.
//!
//! - [`nostate`]: the pre-state phase (`AppBuilder<NoState>`) — bean/producer
//!   registration, config loading, pre-state plugins, `build_state`.
//! - [`typed`]: the typed phase (`AppBuilder<T>`) — controllers, plugins,
//!   layers, hooks, `build()` / `prepare()` / `serve()`.
//! - [`prepared`]: [`PreparedApp`] + the serving lifecycle (`run()`).
//! - [`task_registry`]: [`TaskRegistryHandle`] shared by scheduler/gRPC/plugins.

mod nostate;
mod prepared;
mod registration;
mod task_registry;
mod typed;

pub use prepared::PreparedApp;
pub use registration::{RegisterController, RegisterControllers, RegisterModule};
pub use task_registry::{ScheduledTaskMarker, TaskRegistryHandle};

use crate::beans::{AsyncBean, Bean, BeanRegistry, Producer, Registrable};
use crate::controller::Controller;
use crate::module::{
    BeanList, ControllerDepsList, ExportsProvided, FeatureModule, ModuleDepsSatisfied, ModuleList,
    ModuleScope,
};
use crate::lifecycle::{ShutdownHook, StartupHook};
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
/// `Provisions` and `Required` lists are appended to `P` and `R`.
pub type WithPluginInstalled<Pl, P, R, Mods> = AppBuilder<
    NoState,
    <P as TAppend<<Pl as RawPreStatePlugin>::Provisions>>::Output,
    <R as TAppend<<Pl as RawPreStatePlugin>::Required>>::Output,
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

type LayerFn = Box<dyn FnOnce(crate::http::Router) -> crate::http::Router + Send>;

/// A meta consumer that drains typed metadata from the registry and returns
/// a router fragment to be merged into the application.
type MetaConsumer<T> = Box<dyn FnOnce(&MetaRegistry) -> crate::http::Router<T> + Send>;

/// A serve hook that receives the shared task registry and a cancellation
/// token. Each hook is responsible for draining the registry of tasks it
/// owns (via `TaskRegistryHandle::take_of::<Tag>()` for tagged tasks, or
/// `take_all()` for single-consumer subsystems).
type ServeHook = Box<dyn FnOnce(TaskRegistryHandle, CancellationToken) + Send>;

/// Shared collection of JobHandles for services spawned via
/// [`AppBuilder::spawn_service`], so shutdown can await their completion
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

/// Resolve the active profile: `R2E_PROFILE` env > `r2e.profile` config > `"default"`.
fn resolve_profile(config: &crate::config::R2eConfig) -> String {
    std::env::var("R2E_PROFILE")
        .ok()
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
    bean_registry: BeanRegistry,
    /// Deferred actions to be executed after state resolution.
    deferred_actions: Vec<DeferredAction>,
    /// Plugin data storage (type-erased, keyed by TypeId).
    plugin_data: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    /// Name of the last plugin that should be installed last (for ordering validation).
    last_plugin_name: Option<&'static str>,
    /// Whether to install a trailing-slash normalization fallback.
    normalize_path: bool,
    /// Whether the DevReload plugin has been applied (prevents double-install).
    dev_reload_applied: bool,
    /// Maximum time allowed for shutdown hooks to complete before force-exiting.
    /// `None` means wait indefinitely (default).
    shutdown_grace_period: Option<Duration>,
    /// Active profile name, resolved from `R2E_PROFILE` env var, `r2e.profile`
    /// config key, or `"default"`.
    active_profile: String,
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
    meta_registry: MetaRegistry,
    meta_consumers: Vec<MetaConsumer<T>>,
    consumer_registrations: Vec<ConsumerReg<T>>,
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
}

impl Default for AppBuilder<NoState, TNil, TNil, TNil> {
    fn default() -> Self {
        Self::new()
    }
}
