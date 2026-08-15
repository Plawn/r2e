//! Plugin system for R2E.
//!
//! Plugins are composable units of functionality installed into an
//! [`AppBuilder`].
//!
//! # Two plugin traits
//!
//! - [`PreStatePlugin`]: For plugins that provide beans (like Scheduler).
//!   Installed with `.plugin(p)` **before** `build_state()`. A pre-state
//!   plugin is one async, fallible [`build`](PreStatePlugin::build) factory
//!   for its `Provided` tuple, executed inside `build_state()` as a node of
//!   the bean graph — dependencies arrive constructed, config arrives loaded.
//! - [`Plugin`]: For plugins that don't provide beans. Installed with
//!   `.with(p)` **after** `build_state()`, with full router access.

use crate::builder::{AppBuilder, NoState};
use crate::type_list::{PluginDeps, PluginProvisions, TAppend};
use std::any::Any;

// ── Post-state Plugin trait ────────────────────────────────────────────────

/// A composable unit of functionality that can be installed into an [`AppBuilder`].
///
/// Plugins are installed after `build_state()` is called. They can:
/// - Add layers to the router
/// - Register routes
/// - Register startup/shutdown hooks
///
/// For plugins that need to provide beans (like Scheduler), use [`PreStatePlugin`]
/// instead.
///
/// # Example
///
/// ```ignore
/// use r2e_core::Plugin;
///
/// pub struct Health;
///
/// impl Plugin for Health {
///     fn install<T: Clone + Send + Sync + 'static>(self, app: AppBuilder<T>) -> AppBuilder<T> {
///         app.register_routes(Router::new().route("/health", get(|| async { "OK" })))
///     }
/// }
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not implement `Plugin`, the post-state plugin API used by `.with()`",
    label = "`.with()` needs a post-state `Plugin`",
    note = "if `{Self}` is a pre-state plugin (it provides beans), install it with `.plugin({Self})` BEFORE `build_state()` instead of `.with({Self})`"
)]
pub trait Plugin: Send + 'static {
    /// Install this plugin into the given `AppBuilder`, returning the modified builder.
    fn install<T: Clone + Send + Sync + 'static>(self, app: AppBuilder<T>) -> AppBuilder<T>;

    /// Whether this plugin should be installed last in the layer stack.
    ///
    /// Some plugins need to be the outermost layer (installed last) to work
    /// correctly. When `should_be_last()` returns `true`, the builder will
    /// warn if other plugins are added after this one.
    fn should_be_last() -> bool
    where
        Self: Sized,
    {
        false
    }

    /// The name of this plugin (for diagnostics).
    fn name() -> &'static str
    where
        Self: Sized,
    {
        std::any::type_name::<Self>()
    }
}

// ── Pre-state Plugin traits ────────────────────────────────────────────────

/// Context passed to [`PreStatePlugin::setup`] — the **rare** pre-graph
/// escape hatch that runs at `.plugin()` time, before the bean graph exists.
///
/// Most plugins never touch it: the plugin's real work happens in
/// [`PreStatePlugin::build`], which runs inside `build_state()` with resolved
/// dependencies and loaded config. Reach for `setup` only for things other
/// **pre-state** code must observe before the graph is built — buffering an
/// early effect, or registering a [`PreDestroy`](crate::PreDestroy) disposer
/// via [`run_pre_destroy`](Self::run_pre_destroy).
///
/// Config is deliberately **not** available here (it may not be loaded yet —
/// `.plugin()` / `load_config` order does not matter). Read config in
/// [`build`](crate::PreStatePlugin::build) instead, where the typed
/// [`Config`](crate::PreStatePlugin::Config) section is guaranteed loaded.
pub struct PluginSetupContext {
    deferred: Vec<DeferredAction>,
    /// Buffered sugar calls ([`add_layer`](Self::add_layer),
    /// [`on_serve`](Self::on_serve), etc.). Flushed as ONE [`DeferredAction`]
    /// by the blanket `RawPreStatePlugin` impl — see [`flush`](Self::flush).
    sugar: Vec<Box<dyn FnOnce(&mut DeferredContext) + Send>>,
    /// Lifecycle-hook registrars applied to the bean registry at `.plugin()`
    /// time. Backs [`run_pre_destroy`](Self::run_pre_destroy).
    registry_ops: Vec<Box<dyn FnOnce(&mut crate::beans::BeanRegistry) + Send>>,
}

impl PluginSetupContext {
    /// Create a new setup context.
    pub(crate) fn new() -> Self {
        Self {
            deferred: Vec::new(),
            sugar: Vec::new(),
            registry_ops: Vec::new(),
        }
    }

    /// Register a [`PreDestroy`](crate::PreDestroy) disposal hook for one of
    /// this plugin's `Provided` beans, run during graceful shutdown.
    ///
    /// See [`AppBuilder::provide_with_pre_destroy`](crate::AppBuilder::provide_with_pre_destroy)
    /// for the invocation order.
    pub fn run_pre_destroy<B: crate::PreDestroy>(&mut self) {
        self.registry_ops
            .push(Box::new(|reg| reg.register_pre_destroy::<B>()));
    }

    /// Drain the buffered bean-registry lifecycle registrars (internal).
    pub(crate) fn take_registry_ops(
        &mut self,
    ) -> Vec<Box<dyn FnOnce(&mut crate::beans::BeanRegistry) + Send>> {
        std::mem::take(&mut self.registry_ops)
    }

    /// Register a deferred action to run after state resolution.
    ///
    /// This is the low-level escape hatch. Most plugins should prefer the
    /// direct sugar methods ([`add_layer`](Self::add_layer),
    /// [`on_serve`](Self::on_serve), [`store_data`](Self::store_data), …),
    /// which buffer their calls and are flushed as a **single** deferred
    /// action.
    ///
    /// # Ordering
    ///
    /// Every action added here runs **before** the sugar-buffered action, in
    /// the order added. The sugar calls are then applied as one final action.
    /// If you need sugar and explicit actions to interleave differently, put
    /// all your logic inside explicit `add_deferred` actions.
    ///
    /// Across plugins, deferred work runs **grouped per plugin, in install
    /// order**: `[A.explicit…, A.sugar, A.build-effects, B.explicit…,
    /// B.sugar, B.build-effects]`. Note that plugin **build** execution
    /// follows the graph's topological order instead — effects and builds
    /// are ordered independently.
    pub fn add_deferred(&mut self, action: DeferredAction) {
        self.deferred.push(action);
    }

    // ── Sugar: direct post-state actions ────────────────────────────────────
    //
    // These mirror `DeferredContext`'s surface but take plain closures — the
    // boxing happens inside. Calls are buffered and flushed as ONE deferred
    // action (named after the plugin type), running after any explicit
    // `add_deferred` actions. Within the flushed action, sugar calls run in
    // the order you made them.

    /// Add a layer to the router (post-state). Sugar for a
    /// [`DeferredContext::add_layer`] call — pass a plain closure, no `Box`.
    ///
    /// Buffered; see the ordering note on [`add_deferred`](Self::add_deferred).
    pub fn add_layer<F>(&mut self, layer: F)
    where
        F: FnOnce(crate::http::Router) -> crate::http::Router + Send + 'static,
    {
        self.sugar
            .push(Box::new(move |dctx| dctx.add_layer(Box::new(layer))));
    }

    /// Add a transport-level router transform applied **outermost**. Sugar for
    /// a [`DeferredContext::wrap_router`] call — pass a plain closure, no `Box`.
    ///
    /// Buffered; see the ordering note on [`add_deferred`](Self::add_deferred).
    pub fn wrap_router<F>(&mut self, wrap: F)
    where
        F: FnOnce(crate::http::Router) -> crate::http::Router + Send + 'static,
    {
        self.sugar
            .push(Box::new(move |dctx| dctx.wrap_router(Box::new(wrap))));
    }

    /// Store plugin-specific data for later retrieval. Sugar for a
    /// [`DeferredContext::store_data`] call.
    ///
    /// Buffered; see the ordering note on [`add_deferred`](Self::add_deferred).
    pub fn store_data<D: Any + Send + Sync + 'static>(&mut self, data: D) {
        self.sugar.push(Box::new(move |dctx| dctx.store_data(data)));
    }

    /// Add a serve hook that runs when the server starts. Sugar for a
    /// [`DeferredContext::on_serve`] call.
    ///
    /// Buffered; see the ordering note on [`add_deferred`](Self::add_deferred).
    pub fn on_serve<F>(&mut self, hook: F)
    where
        F: FnOnce(crate::builder::ServeContext) + Send + 'static,
    {
        self.sugar.push(Box::new(move |dctx| dctx.on_serve(hook)));
    }

    /// Add a shutdown hook that runs when the server stops. Sugar for a
    /// [`DeferredContext::on_shutdown`] call.
    ///
    /// Buffered; see the ordering note on [`add_deferred`](Self::add_deferred).
    pub fn on_shutdown<F>(&mut self, hook: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.sugar
            .push(Box::new(move |dctx| dctx.on_shutdown(hook)));
    }

    /// Add an async shutdown hook awaited during shutdown. Sugar for a
    /// [`DeferredContext::on_shutdown_async`] call.
    ///
    /// Buffered; see the ordering note on [`add_deferred`](Self::add_deferred).
    pub fn on_shutdown_async<F, Fut>(&mut self, hook: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        self.sugar
            .push(Box::new(move |dctx| dctx.on_shutdown_async(hook)));
    }

    /// Consume the context, returning the deferred actions to install.
    ///
    /// Actions added via [`add_deferred`](Self::add_deferred) come first, in
    /// call order; the buffered sugar calls are appended as a **single**
    /// [`DeferredAction`] named `name` (typically the plugin's short type name,
    /// via [`plugin_action_name`]). Empty sugar contributes no action.
    pub(crate) fn flush(self, name: &'static str) -> Vec<DeferredAction> {
        let PluginSetupContext {
            mut deferred,
            sugar,
            ..
        } = self;
        if !sugar.is_empty() {
            deferred.push(DeferredAction::new(name, move |dctx| {
                for op in sugar {
                    op(dctx);
                }
            }));
        }
        deferred
    }
}

/// Derive a short, human-readable action name from a plugin type — the last
/// path segment of its type name, before any generic arguments. For example
/// `r2e_prometheus::Prometheus` → `"Prometheus"`.
///
/// Used by the blanket [`RawPreStatePlugin`] impl to name the single
/// [`DeferredAction`] flushed from a plugin's buffered sugar calls, so the
/// plugin author never has to name it themselves.
#[doc(hidden)]
pub fn plugin_action_name<T: ?Sized>() -> &'static str {
    let full = std::any::type_name::<T>();
    let base = full.split('<').next().unwrap_or(full);
    let short = base.rsplit("::").next().unwrap_or(base);
    if short.is_empty() {
        full
    } else {
        short
    }
}

/// A plugin that runs in the pre-state phase and provides beans.
///
/// A pre-state plugin **is one async, fallible factory** for its
/// [`Provided`](Self::Provided) tuple: [`build`](Self::build) runs inside
/// [`build_state()`](crate::AppBuilder::build_state) as a node of the bean
/// graph, topologically ordered after its [`Deps`](Self::Deps), with the
/// typed [`Config`](Self::Config) section guaranteed loaded. Writing a plugin
/// feels like writing a `#[bean]` constructor — no shells, no two-phase
/// install/configure dance.
///
/// ```text
///   .plugin(Me)                 build_state()                (serve)
///        │                           │                          │
///        ▼                           ▼                          ▼
///   [node queued]   ─►  deps built ─► build(deps, config) ─► effects applied
/// ```
///
/// Each element of the `Provided` tuple is projected into the graph as its
/// own bean, so other beans, controllers, and plugins inject them normally —
/// and their construction is a **real** topological edge: a `#[bean]` that
/// injects a plugin-provided type is built after the plugin, and vice versa.
///
/// # Dependencies
///
/// Declare dependencies via [`Deps`](Self::Deps); they arrive **by value** in
/// [`build`](Self::build), already constructed. They are also appended to the
/// builder's requirement list and verified against the **final** provision
/// list at `build_state()` — order-independent, so a dependency may be
/// `.provide()`-d or `.register()`-ed before *or after* the `.plugin()` call:
///
/// ```ignore
/// AppBuilder::new()
///     .plugin(MyPlugin { .. })    // Deps = (DbPool,)
///     .provide(pool)              // ✅ provided after the plugin — fine
///     .build_state().await        // ❌ compile error HERE if DbPool is missing
/// ```
///
/// [`Provided`](Self::Provided) is a **tuple** of beans: `(A,)` for a single
/// bean, `(A, B)` for several, or `()` for none.
///
/// # Effects
///
/// Router layers, serve/shutdown hooks, and plugin data are registered on the
/// [`PluginBuildContext`] during `build` and applied after the graph is
/// resolved. Effects are applied **in plugin install order** (while builds
/// execute in topological order), and are dropped when the plugin is disabled
/// via `<prefix>.enabled = false`.
///
/// For plugins that need arbitrary builder access (calling `.register()`,
/// `.provide()`, etc. by hand), implement [`RawPreStatePlugin`] instead — but
/// that is rarely necessary. Every `PreStatePlugin` is automatically a
/// [`RawPreStatePlugin`] via a blanket impl, so both work with `.plugin()`.
///
/// # Examples
///
/// Simple plugin (no dependencies):
///
/// ```ignore
/// use r2e_core::{PreStatePlugin, PluginBuildContext, PluginBuildError};
///
/// pub struct MyPlugin { pub value: String }
///
/// impl PreStatePlugin for MyPlugin {
///     type Provided = (String,);
///     type Deps = ();
///     type Config = ();
///
///     async fn build(
///         self,
///         _deps: (),
///         _config: Option<()>,
///         _ctx: &mut PluginBuildContext,
///     ) -> Result<(String,), PluginBuildError> {
///         Ok((self.value,))
///     }
/// }
/// ```
///
/// Plugin whose bean is built from dependencies, with effects:
///
/// ```ignore
/// impl PreStatePlugin for MyPlugin {
///     type Provided = (MyService,);
///     type Deps = (DbPool, CancellationToken);
///     type Config = MyConfig;               // #[derive(ConfigProperties)]
///     const CONFIG_PREFIX: Option<&'static str> = Some("my-plugin");
///
///     async fn build(
///         self,
///         (pool, token): (DbPool, CancellationToken),
///         config: Option<MyConfig>,
///         ctx: &mut PluginBuildContext,
///     ) -> Result<(MyService,), PluginBuildError> {
///         let service = MyService::connect(pool, config.unwrap_or_default()).await?;
///         let handle = service.handle();
///         ctx.on_shutdown_async(move || async move { handle.drain().await });
///         Ok((service,))
///     }
/// }
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not implement a pre-state plugin trait (`PreStatePlugin`/`RawPreStatePlugin`), the API used by `.plugin()`",
    label = "`.plugin()` needs a pre-state plugin",
    note = "if `{Self}` is a post-state plugin, install it with `.with({Self})` AFTER `build_state()` instead of `.plugin({Self})`"
)]
pub trait PreStatePlugin: Send + Sized + 'static {
    /// The **tuple** of bean types this plugin provides to the bean graph.
    ///
    /// Use `(A,)` for a single bean, `(A, B)` for several, or `()` for none.
    /// Each element must be `Clone + Send + Sync + 'static`; each becomes its
    /// own bean in the graph, projected out of the tuple [`build`](Self::build)
    /// returns.
    type Provided: PluginProvisions;

    /// Bean dependencies this plugin requires, as a concrete tuple —
    /// constructed **before** [`build`](Self::build) (they are real edges in
    /// the bean graph's topological order) and handed to it by value.
    ///
    /// These may name **any** bean — `.provide()`-d values, `.register::<T>()`-ed
    /// (factory-built) beans, and beans other plugins provide. They are
    /// appended to the builder's requirement list and verified against the
    /// **final** provision list at `build_state()` (not at the `.plugin()`
    /// call site), so a dependency may be supplied *after* this plugin is
    /// installed.
    ///
    /// Most plugins set this to `()` (no dependencies). On stable Rust
    /// associated types have no defaults, so every impl must write it
    /// explicitly:
    ///
    /// ```ignore
    /// type Deps = ();
    /// ```
    type Deps: crate::type_list::PluginDeps;

    /// The plugin's typed configuration section.
    ///
    /// Set to `()` (the common case) for a plugin that reads no typed config —
    /// it can still read raw keys via [`PluginBuildContext::config_raw`]. For
    /// typed config, set this to any `#[derive(ConfigProperties)]` struct and
    /// point [`CONFIG_PREFIX`](Self::CONFIG_PREFIX) at its YAML section. The
    /// framework loads and validates that section before calling
    /// [`build`](Self::build) — a malformed value produces the same boot error
    /// a controller's `#[config(section)]` mismatch does.
    ///
    /// On stable Rust associated types have no defaults, so every impl must
    /// write it explicitly (`type Config = ();`).
    type Config: crate::config::PluginConfig;

    /// The YAML section prefix for [`Config`](Self::Config).
    ///
    /// `None` (the default) disables typed-config loading — use it with
    /// `type Config = ();`. `Some("prometheus")` loads the `Config` from the
    /// `prometheus.*` section. The section is treated as **optional**
    /// (presence-based, like a controller's `Option<Section>`): if no config
    /// was loaded, or no key lives under the prefix, [`build`](Self::build)
    /// receives `None`. A present-but-invalid section is a boot error. The
    /// section is parsed whenever it is present — **even when the plugin is
    /// disabled** via `<prefix>.enabled = false` — so it is always structurally
    /// validated; keep semantic (cross-field) validation behind your own
    /// enabled check if it must not fire when disabled.
    const CONFIG_PREFIX: Option<&'static str> = None;

    /// Optional dev-reload stamp mixed into the plugin node's build fingerprint.
    ///
    /// Plugin nodes are volatile — rebuilt on every `r2e dev` hot-patch cycle —
    /// so most plugins never touch this. It exists as a forward-compatible
    /// escape hatch should reuse semantics ever change.
    const BUILD_VERSION: u64 = 0;

    /// Rare **pre-graph** escape hatch, run once at `.plugin()` time.
    /// Default: no-op.
    ///
    /// Runs before the bean graph exists — and possibly before config is
    /// loaded — so nothing is resolvable here. Use it only for things other
    /// pre-state code must observe (see [`PluginSetupContext`]): lifecycle
    /// registrars, explicit low-level deferred actions. Everything else —
    /// building beans, reading config, registering layers and hooks — belongs
    /// in [`build`](Self::build).
    #[allow(unused_variables)]
    fn setup(&mut self, ctx: &mut PluginSetupContext) {}

    /// Build the plugin's [`Provided`](Self::Provided) beans — **the** plugin.
    ///
    /// Runs inside `build_state()` as a node of the bean graph, topologically
    /// after [`Deps`](Self::Deps) (which arrive fully constructed, by value),
    /// with config guaranteed loaded. May be `async` and may fail: an `Err`
    /// aborts startup with a
    /// [`BeanError::PluginBuild`](crate::beans::BeanError::PluginBuild) naming
    /// the plugin. Side effects (router layers, serve/shutdown hooks, plugin
    /// data) are registered on `ctx` — see [`PluginBuildContext`].
    ///
    /// # `config`
    ///
    /// `Some(cfg)` only when [`CONFIG_PREFIX`](Self::CONFIG_PREFIX) is
    /// `Some(prefix)`, config was loaded, and a key lives under that prefix;
    /// otherwise `None`. Precedence for a config-consuming plugin is: explicit
    /// builder setting (a field on `self`) > `config` (file) > built-in
    /// default.
    ///
    /// # Disabled plugins
    ///
    /// `build` **always runs**, even when `<prefix>.enabled = false` — the
    /// `Provided` types are part of the compile-time provision list, so the
    /// beans must exist. Check [`ctx.enabled()`](PluginBuildContext::enabled)
    /// and return a cheap disabled variant; effects registered on `ctx` are
    /// dropped automatically when the plugin is disabled.
    ///
    /// # Test pinning
    ///
    /// When a test pins **every** `Provided` type before install
    /// (`override_bean` for each), `build` is skipped entirely. Pinning only
    /// *some* of them still runs `build` (the group node yields the whole
    /// tuple) while the pinned types keep their overrides in the graph; to
    /// also silence such a plugin's side effects, disable it via
    /// `<prefix>.enabled = false`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// impl PreStatePlugin for MetricsExporter {
    ///     type Provided = (ExporterHandle,);
    ///     type Deps = (MetricsRegistry,); // registered elsewhere via `.register()`
    ///     type Config = ExporterConfig;
    ///     const CONFIG_PREFIX: Option<&'static str> = Some("exporter");
    ///
    ///     async fn build(
    ///         self,
    ///         (registry,): (MetricsRegistry,),
    ///         config: Option<ExporterConfig>,
    ///         ctx: &mut PluginBuildContext,
    ///     ) -> Result<(ExporterHandle,), PluginBuildError> {
    ///         let handle = ExporterHandle::connect(config.unwrap_or_default()).await?;
    ///         let h = handle.clone();
    ///         ctx.on_serve(move |_sc| h.bind(registry));
    ///         Ok((handle,))
    ///     }
    /// }
    /// ```
    fn build(
        self,
        deps: Self::Deps,
        config: Option<Self::Config>,
        ctx: &mut PluginBuildContext,
    ) -> impl std::future::Future<Output = Result<Self::Provided, PluginBuildError>> + Send;
}

/// The error type [`PreStatePlugin::build`] returns; an `Err` aborts boot with
/// a [`BeanError::PluginBuild`](crate::beans::BeanError::PluginBuild).
pub type PluginBuildError = Box<dyn std::error::Error + Send + Sync>;

/// Internal machinery backing [`PreStatePlugin`] — **not** part of the public
/// plugin-authoring surface.
///
/// This trait is the HList-based, full-builder-access form that `.plugin()`
/// actually dispatches on. Every [`PreStatePlugin`] gets a `RawPreStatePlugin`
/// impl for free via the blanket impl below, which is how multi-bean plugins,
/// deferred actions, and compile-time dependency checking are wired into the
/// builder's type-level provision/requirement lists.
///
/// **Almost no one should implement this directly.** The simplified
/// [`PreStatePlugin`] now supports multiple provided beans (via a tuple
/// [`Provided`](PreStatePlugin::Provided)), so the only remaining reason to
/// hand-write a `RawPreStatePlugin` is to call arbitrary builder methods
/// (`.register()`, `.provide()`, `.when()`, …) during install. It is kept as an
/// escape hatch for that case.
///
/// # `Required = TNil` and `with_updated_types()`
///
/// When `Required` is `TNil`, the compiler cannot prove that
/// `<R as TAppend<TNil>>::Output == R`. Since `R` is a phantom type parameter,
/// call [`.with_updated_types()`](AppBuilder::with_updated_types) at the end of
/// `install()` to perform the zero-cost phantom type conversion.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not implement a pre-state plugin trait (`PreStatePlugin`/`RawPreStatePlugin`), the API used by `.plugin()`",
    label = "`.plugin()` needs a pre-state plugin",
    note = "if `{Self}` is a post-state plugin, install it with `.with({Self})` AFTER `build_state()` instead of `.plugin({Self})`"
)]
pub trait RawPreStatePlugin: Send + 'static {
    /// The type-level list of bean types this plugin provides.
    ///
    /// For a single provision: `TCons<MyType, TNil>`.
    /// For multiple: `TCons<A, TCons<B, TNil>>`.
    type Provisions;

    /// The requirement list appended to the builder's `R`: the plugin's `Deps`.
    ///
    /// Nothing is checked at the `.plugin()` call site — the list rides along
    /// in `R` and is verified against the **final** provision list at
    /// `build_state()`, so a dependency may be provided or registered *after*
    /// this plugin is installed.
    type Required;

    /// Install the plugin in the pre-state phase with full builder access.
    ///
    /// `Mods` is the builder's pending feature-module list — plugins carry it
    /// through unchanged.
    fn install<P, R, Mods>(
        self,
        app: AppBuilder<NoState, P, R, Mods>,
    ) -> crate::builder::WithPluginInstalled<Self, P, R, Mods>
    where
        P: TAppend<Self::Provisions>,
        R: TAppend<Self::Required>;
}

// Blanket impl: every PreStatePlugin is automatically a RawPreStatePlugin.
//
// The plugin's `Provided` tuple maps to the type-level provision list via
// `PluginProvisions::AsList`. At the value level, the plugin becomes bean-graph
// nodes: one **group node** (`PluginOut<T>`, running `build` with resolved deps
// and loaded config) plus one **projection node** per `Provided` element that
// clones its slot out of the group's tuple. The type-level list is then
// advanced in one phantom `with_updated_types()` cast. Projections register
// strict — colliding with an app `.provide()`/`.register()` of the same type
// (or installing the same plugin twice) is a `DuplicateBean` boot error, and a
// `pin_provide` override placed BEFORE `.plugin()` wins per type.
impl<T> RawPreStatePlugin for T
where
    T: PreStatePlugin,
{
    type Provisions = <T::Provided as PluginProvisions>::AsList;
    type Required = <T::Deps as PluginDeps>::AsList;

    fn install<P, R, Mods>(
        self,
        app: AppBuilder<NoState, P, R, Mods>,
    ) -> crate::builder::WithPluginInstalled<Self, P, R, Mods>
    where
        P: TAppend<Self::Provisions>,
        R: TAppend<Self::Required>,
    {
        let name = plugin_action_name::<T>();
        let prefix = T::CONFIG_PREFIX;
        let mut plugin = self;
        let (registry_ops, deferred) = {
            let mut ctx = PluginSetupContext::new();
            plugin.setup(&mut ctx);
            let registry_ops = ctx.take_registry_ops();
            (registry_ops, ctx.flush(name))
        };
        let mut builder = app;
        for action in deferred {
            // Gate every buffered/explicit setup action on `<prefix>.enabled`.
            builder = builder.add_deferred(gate_on_enabled(action, prefix));
        }
        // Apply lifecycle registrars (run_pre_destroy) the plugin opted its
        // `Provided` beans into. NOT gated by `enabled`: the beans still
        // exist, so their lifecycle stays honest.
        for op in registry_ops {
            op(builder.bean_registry_mut());
        }
        // All-pinned skip: when a test pins EVERY provided type before
        // `.plugin()` (whole-plugin mock), neither `build` nor its effects
        // should run — register nothing. An empty `Provided` tuple must NOT
        // take this path (`all()` on an empty iterator is vacuously true, but
        // a `Provided = ()` plugin still runs `build` for its effects).
        let ids = <T::Provided as PluginProvisions>::element_ids();
        let all_pinned = {
            let registry = builder.bean_registry_mut();
            !ids.is_empty() && ids.iter().all(|(tid, _)| registry.is_pinned(tid))
        };
        if !all_pinned {
            // Group node (runs `build`) + per-element projections. Effects
            // registered on the PluginBuildContext land in this shared slot,
            // drained by the deferred action below.
            let effects = EffectsSlot::default();
            builder
                .bean_registry_mut()
                .register_plugin_group(plugin, effects.clone());
            // Effects apply at the plugin's install-order slot (builds execute
            // in topological order — the two orders are independent). This
            // action is also the single place the "disabled" diagnostic is
            // emitted, since exactly one is scheduled per plugin.
            builder = builder.add_deferred(DeferredAction::new(name, move |dctx| {
                if !plugin_config_enabled(dctx.config(), prefix) {
                    tracing::info!(
                        plugin = name,
                        "plugin disabled via `{}.enabled = false`; effects skipped (its beans remain in the graph)",
                        prefix.unwrap_or(name),
                    );
                    return;
                }
                for op in effects.drain() {
                    op(dctx);
                }
            }));
        }
        builder.with_updated_types()
    }
}

/// Load and validate a plugin's typed [`Config`](PreStatePlugin::Config)
/// section from an already-resolved `R2eConfig` (the graph-provided bean),
/// at plugin-build time.
///
/// The section is optional (presence-based, like a controller's
/// `Option<Section>`): returns `None` when config loading is disabled
/// (`CONFIG_PREFIX == None`), no config was loaded, or no key lives under the
/// prefix. A present-but-invalid section panics with the same validation report
/// a controller `#[config]` mismatch produces (`plugin` names the plugin in the
/// message) — even when the plugin is disabled via `<prefix>.enabled = false`.
pub(crate) fn load_plugin_config_from<T: PreStatePlugin>(
    config: Option<&crate::config::R2eConfig>,
    plugin: &str,
) -> Option<T::Config> {
    use crate::config::PluginConfig;

    let prefix = T::CONFIG_PREFIX?;
    let config = config?;
    if !config.has_prefix(prefix) {
        return None;
    }
    let errors = <T::Config as PluginConfig>::plugin_validate(config, prefix);
    if !errors.is_empty() {
        panic!(
            "Invalid configuration for plugin `{plugin}` (section `{prefix}`):\n{}",
            crate::config::ConfigValidationError { errors }
        );
    }
    Some(
        <T::Config as PluginConfig>::plugin_load(config, prefix)
            .expect("plugin config section validated but failed to construct"),
    )
}

// ── Plugin build machinery ─────────────────────────────────────────────────

/// A cheap, cloneable, **deferred-fill** handle on the final resolved bean
/// graph.
///
/// Handed to plugins via [`PluginBuildContext::graph`] while the graph is
/// still being built; the framework fills it right after `build_state()`
/// resolves, so [`get`](Self::get) returns `Some` from any code running after
/// resolution (serve hooks, request handlers, background tasks). Reading it
/// **during** a plugin's `build` returns `None` — take dependencies through
/// [`Deps`](PreStatePlugin::Deps) instead; the handle exists for values that
/// must resolve beans lazily *after* boot (per-tenant sources, resource
/// factories).
#[derive(Clone, Default)]
pub struct GraphHandle(crate::late::Late<std::sync::Arc<crate::beans::BeanContext>>);

impl GraphHandle {
    /// Create an empty (unfilled) handle. Internal — the builder owns filling.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Fill the handle with the resolved graph. First write wins; later calls
    /// are ignored (relevant across dev-reload cycles, where the registry —
    /// and thus the handle — is fresh per cycle anyway).
    ///
    /// The builder does this for you after `build_state()`. It is public for
    /// embedders that build a `BeanContext` by hand (tests, hand-wired
    /// per-tenant maps) and need to satisfy an API that takes a `GraphHandle`:
    /// start from [`GraphHandle::default`], hand out clones, fill once.
    pub fn fill(&self, ctx: std::sync::Arc<crate::beans::BeanContext>) {
        let _ = self.0.fill(ctx);
    }

    /// The resolved bean graph, or `None` before `build_state()` completes.
    pub fn get(&self) -> Option<&std::sync::Arc<crate::beans::BeanContext>> {
        self.0.get()
    }

    /// Resolve a bean from the graph, or `None` before resolution / when the
    /// bean is absent.
    pub fn bean<B: Clone + Send + Sync + 'static>(&self) -> Option<B> {
        self.get().and_then(|ctx| ctx.try_get::<B>())
    }
}

impl std::fmt::Debug for GraphHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphHandle")
            .field("filled", &self.get().is_some())
            .finish()
    }
}

/// Shared buffer between a plugin's group-node factory (which fills it during
/// `build`) and the plugin's install-order deferred action (which drains and
/// applies it — or drops it when the plugin is disabled).
#[derive(Clone, Default)]
pub(crate) struct EffectsSlot(
    std::sync::Arc<std::sync::Mutex<Vec<Box<dyn FnOnce(&mut DeferredContext) + Send>>>>,
);

impl EffectsSlot {
    pub(crate) fn fill(&self, effects: Vec<Box<dyn FnOnce(&mut DeferredContext) + Send>>) {
        *self.0.lock().expect("EffectsSlot poisoned") = effects;
    }

    pub(crate) fn drain(&self) -> Vec<Box<dyn FnOnce(&mut DeferredContext) + Send>> {
        std::mem::take(&mut *self.0.lock().expect("EffectsSlot poisoned"))
    }
}

/// Context passed to [`PreStatePlugin::build`] — effect registration plus the
/// build-time environment (enabled flag, raw config, graph handle).
///
/// Owned by the group node's factory future (no borrows), so `build` can be a
/// real `async fn`. Effects buffered here are applied after graph resolution,
/// at the plugin's install-order slot — and **dropped** when the plugin is
/// disabled via `<prefix>.enabled = false`.
pub struct PluginBuildContext {
    enabled: bool,
    graph: GraphHandle,
    config: Option<crate::config::R2eConfig>,
    effects: Vec<Box<dyn FnOnce(&mut DeferredContext) + Send>>,
}

impl PluginBuildContext {
    pub(crate) fn new(
        enabled: bool,
        graph: GraphHandle,
        config: Option<crate::config::R2eConfig>,
    ) -> Self {
        Self {
            enabled,
            graph,
            config,
            effects: Vec::new(),
        }
    }

    pub(crate) fn into_effects(self) -> Vec<Box<dyn FnOnce(&mut DeferredContext) + Send>> {
        self.effects
    }

    /// Whether the plugin is enabled (`<prefix>.enabled != false`).
    ///
    /// [`build`](PreStatePlugin::build) always runs — check this and return a
    /// cheap disabled variant of your beans when `false`. Effects registered
    /// on this context are dropped automatically when disabled.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// A deferred-fill handle on the final resolved bean graph.
    ///
    /// Empty during `build` (the graph is still being built — take
    /// dependencies through [`Deps`](PreStatePlugin::Deps)); filled right
    /// after `build_state()` resolves. Store it in beans that must resolve
    /// other beans lazily after boot (per-tenant sources, resource factories).
    pub fn graph(&self) -> GraphHandle {
        self.graph.clone()
    }

    /// The loaded [`R2eConfig`](crate::config::R2eConfig), if any — the
    /// low-level counterpart to the typed [`Config`](PreStatePlugin::Config)
    /// parameter, for reading keys outside the plugin's own section.
    pub fn config_raw(&self) -> Option<&crate::config::R2eConfig> {
        self.config.as_ref()
    }

    /// Escape hatch: run a closure against the full [`DeferredContext`] after
    /// graph resolution (at the plugin's install-order effect slot). Prefer
    /// the dedicated sugar methods; use this for anything they don't cover
    /// (e.g. `take_data`).
    ///
    /// Dropped (never run) when the plugin is disabled.
    pub fn after_build<F>(&mut self, f: F)
    where
        F: FnOnce(&mut DeferredContext) + Send + 'static,
    {
        self.effects.push(Box::new(f));
    }

    // ── Effect sugar (mirrors `DeferredContext`) ────────────────────────────

    /// Add a layer to the router. Applied after graph resolution; dropped when
    /// the plugin is disabled.
    pub fn add_layer<F>(&mut self, layer: F)
    where
        F: FnOnce(crate::http::Router) -> crate::http::Router + Send + 'static,
    {
        self.effects
            .push(Box::new(move |dctx| dctx.add_layer(Box::new(layer))));
    }

    /// Add a transport-level router transform applied **outermost**. See
    /// [`DeferredContext::wrap_router`] for when to prefer it over
    /// [`add_layer`](Self::add_layer).
    pub fn wrap_router<F>(&mut self, wrap: F)
    where
        F: FnOnce(crate::http::Router) -> crate::http::Router + Send + 'static,
    {
        self.effects
            .push(Box::new(move |dctx| dctx.wrap_router(Box::new(wrap))));
    }

    /// Store plugin-specific data for later retrieval (keyed by type). See
    /// [`DeferredContext::store_data`].
    pub fn store_data<D: Any + Send + Sync + 'static>(&mut self, data: D) {
        self.effects
            .push(Box::new(move |dctx| dctx.store_data(data)));
    }

    /// Add a serve hook that runs when the server starts. See
    /// [`DeferredContext::on_serve`].
    pub fn on_serve<F>(&mut self, hook: F)
    where
        F: FnOnce(crate::builder::ServeContext) + Send + 'static,
    {
        self.effects.push(Box::new(move |dctx| dctx.on_serve(hook)));
    }

    /// Add a shutdown hook that runs when the server stops. See
    /// [`DeferredContext::on_shutdown`].
    pub fn on_shutdown<F>(&mut self, hook: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.effects
            .push(Box::new(move |dctx| dctx.on_shutdown(hook)));
    }

    /// Add an async shutdown hook awaited during shutdown. See
    /// [`DeferredContext::on_shutdown_async`].
    pub fn on_shutdown_async<F, Fut>(&mut self, hook: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        self.effects
            .push(Box::new(move |dctx| dctx.on_shutdown_async(hook)));
    }
}

/// The group-node bean for a plugin `Pl`: the whole `Provided` tuple, built by
/// one run of [`PreStatePlugin::build`]. Projection nodes clone individual
/// elements out of it. Internal — never inject this type.
#[doc(hidden)]
pub struct PluginOut<Pl: PreStatePlugin>(pub Pl::Provided);

// Manual impl: `#[derive(Clone)]` would bound `Pl: Clone`, but only the tuple
// needs to be cloneable (and `PluginProvisions: Clone` guarantees it).
impl<Pl: PreStatePlugin> Clone for PluginOut<Pl> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

/// Whether a plugin's post-state effects should run, per the `<prefix>.enabled`
/// convention (phase 6).
///
/// Returns `true` (enabled) when the plugin declares no `CONFIG_PREFIX`, when no
/// config was loaded, or when the `<prefix>.enabled` key is absent — the flag
/// defaults to `true`, so plugins are on unless explicitly turned off. Only an
/// explicit `<prefix>.enabled = false` disables them.
pub(crate) fn plugin_config_enabled(
    config: Option<&crate::config::R2eConfig>,
    prefix: Option<&'static str>,
) -> bool {
    let (Some(prefix), Some(config)) = (prefix, config) else {
        return true;
    };
    config
        .get::<bool>(&format!("{prefix}.enabled"))
        .unwrap_or(true)
}

/// Wrap a plugin-scheduled [`DeferredAction`] so it runs only when the plugin is
/// enabled (`<prefix>.enabled != false`). A disabled plugin's sugar and explicit
/// deferred actions become inert; the "disabled" diagnostic is emitted once from
/// the build-effects action instead (see the blanket `install`).
fn gate_on_enabled(action: DeferredAction, prefix: Option<&'static str>) -> DeferredAction {
    let DeferredAction { name, action } = action;
    DeferredAction::new(name, move |dctx| {
        if plugin_config_enabled(dctx.config(), prefix) {
            action(dctx);
        }
    })
}

// ── Deferred action system ─────────────────────────────────────────────────

/// A deferred action that runs after state resolution.
///
/// This is the low-level mechanism for plugins that need to run setup code
/// after `build_state()` is called. Each action is a closure that receives a
/// `DeferredContext` providing access to builder internals.
///
/// Most plugins never construct one directly: the effect sugar on
/// [`PluginBuildContext`] ([`add_layer`](PluginBuildContext::add_layer),
/// [`on_shutdown`](PluginBuildContext::on_shutdown), …) covers the common
/// cases from inside [`build`](PreStatePlugin::build). Reach for
/// `PluginSetupContext::add_deferred(DeferredAction::new(..))` only as a
/// pre-graph escape hatch.
///
/// # Example (preferred — build-time sugar)
///
/// ```ignore
/// impl PreStatePlugin for MyPlugin {
///     type Provided = (MyToken,);
///     type Deps = ();
///     type Config = ();
///
///     async fn build(
///         self,
///         _deps: (),
///         _config: Option<()>,
///         ctx: &mut PluginBuildContext,
///     ) -> Result<(MyToken,), PluginBuildError> {
///         let token = MyToken::new();
///         let handle = MyHandle::new(token.clone());
///
///         ctx.add_layer(move |router| router.layer(Extension(handle)));
///         ctx.on_shutdown(|| { /* cleanup */ });
///         Ok((token,))
///     }
/// }
/// ```
pub struct DeferredAction {
    /// Name of the action (for debugging/logging).
    pub name: &'static str,
    /// The action to execute.
    pub action: Box<dyn FnOnce(&mut DeferredContext) + Send>,
}

impl DeferredAction {
    /// Create a new deferred action.
    pub fn new<F>(name: &'static str, action: F) -> Self
    where
        F: FnOnce(&mut DeferredContext) + Send + 'static,
    {
        Self {
            name,
            action: Box::new(action),
        }
    }
}

/// A boxed async shutdown hook.
pub type AsyncShutdownHook =
    Box<dyn FnOnce() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send>;

/// Context for executing a deferred action.
///
/// Provides access to builder internals that deferred actions may need to modify.
pub struct DeferredContext<'a> {
    /// Layers to apply to the router.
    #[doc(hidden)]
    pub layers: &'a mut Vec<Box<dyn FnOnce(crate::http::Router) -> crate::http::Router + Send>>,
    /// Transport-level router transforms, applied outermost (after layers and
    /// the catch-panic layer). See [`DeferredContext::wrap_router`].
    #[doc(hidden)]
    pub router_wraps:
        &'a mut Vec<Box<dyn FnOnce(crate::http::Router) -> crate::http::Router + Send>>,
    /// Plugin data storage.
    #[doc(hidden)]
    pub plugin_data:
        &'a mut std::collections::HashMap<std::any::TypeId, Box<dyn Any + Send + Sync>>,
    /// Serve hooks (called when server starts). Each hook receives a
    /// [`ServeContext`](crate::builder::ServeContext) tying it into the
    /// app's shutdown sequence.
    #[doc(hidden)]
    pub serve_hooks: &'a mut Vec<Box<dyn FnOnce(crate::builder::ServeContext) + Send>>,
    /// Shutdown hooks from plugins (sync).
    #[doc(hidden)]
    pub shutdown_hooks: &'a mut Vec<Box<dyn FnOnce() + Send>>,
    /// Shutdown hooks from plugins (async, awaited during shutdown).
    #[doc(hidden)]
    pub async_shutdown_hooks: &'a mut Vec<AsyncShutdownHook>,
    /// The fully resolved bean graph, available because deferred actions run
    /// after `build_state()`. Read beans out of it via
    /// [`bean_context`](DeferredContext::bean_context).
    #[doc(hidden)]
    pub bean_context: &'a std::sync::Arc<crate::beans::BeanContext>,
    /// The loaded [`R2eConfig`](crate::config::R2eConfig), if any. Deferred
    /// actions run inside `build_state()`, which always follows `load_config` /
    /// `with_config`. `None` only when neither `load_config` nor `with_config`
    /// was called.
    #[doc(hidden)]
    pub config: Option<&'a crate::config::R2eConfig>,
}

impl DeferredContext<'_> {
    /// The fully resolved bean graph.
    ///
    /// Deferred actions run after `build_state()`, so every bean —
    /// `.provide()`-d, `.register()`-ed (factory-built), or produced by another
    /// plugin — is materialized and readable here (`ctx.bean_context().get::<T>()`).
    pub fn bean_context(&self) -> &crate::beans::BeanContext {
        self.bean_context
    }

    /// A **retainable** handle on the resolved bean graph.
    ///
    /// [`bean_context`](Self::bean_context) is a borrow, and
    /// `BeanContext::clone()` deliberately keeps only the shared base (it drops
    /// the overlay of factory-built beans). A deferred action that must hand
    /// the graph to something outliving it takes this handle instead, which
    /// keeps the *whole* graph alive. (Plugins should prefer
    /// [`PluginBuildContext::graph`].)
    pub fn bean_context_handle(&self) -> std::sync::Arc<crate::beans::BeanContext> {
        std::sync::Arc::clone(self.bean_context)
    }

    /// The loaded [`R2eConfig`](crate::config::R2eConfig), if any.
    ///
    /// `Some` whenever `load_config` / `with_config` was called (the reliable
    /// point for config — it always precedes `build_state()`). This is the
    /// low-level counterpart to a plugin's typed [`Config`](crate::PreStatePlugin::Config).
    pub fn config(&self) -> Option<&crate::config::R2eConfig> {
        self.config
    }

    /// Add a layer to the router.
    pub fn add_layer(
        &mut self,
        layer: Box<dyn FnOnce(crate::http::Router) -> crate::http::Router + Send>,
    ) {
        self.layers.push(layer);
    }

    /// Add a transport-level router transform, applied **outermost** — after
    /// every [`add_layer`](Self::add_layer) layer (regardless of plugin
    /// install order) and after the built-in catch-panic layer.
    ///
    /// Use this instead of `add_layer` when the transform routes traffic
    /// *around* the HTTP stack (e.g. a content-type multiplexer handing
    /// gRPC requests to tonic): the wrapped-in service sees raw requests
    /// before any HTTP middleware, while the inner HTTP router keeps its
    /// full middleware stack. Do NOT use it for ordinary HTTP middleware —
    /// it would also intercept the non-HTTP branch of any multiplexer
    /// installed by another plugin.
    pub fn wrap_router(
        &mut self,
        wrap: Box<dyn FnOnce(crate::http::Router) -> crate::http::Router + Send>,
    ) {
        self.router_wraps.push(wrap);
    }

    /// Store plugin-specific data for later retrieval.
    ///
    /// Plugins can store arbitrary data keyed by type. This data persists
    /// through the builder lifecycle and can be retrieved during controller
    /// registration or serve hooks.
    pub fn store_data<D: Any + Send + Sync + 'static>(&mut self, data: D) {
        self.plugin_data
            .insert(std::any::TypeId::of::<D>(), Box::new(data));
    }

    /// Remove and return plugin data stored earlier, if present.
    ///
    /// The counterpart of [`store_data`](Self::store_data) /
    /// [`PluginBuildContext::store_data`]: a plugin can stash a non-`Clone`
    /// value and move it out later in an
    /// [`after_build`](crate::plugin::PluginBuildContext::after_build) closure
    /// or another deferred action — e.g. a command channel receiver that must
    /// travel into a serve hook. Returns `None` when no value of type `D` was
    /// stored.
    pub fn take_data<D: Any + Send + Sync + 'static>(&mut self) -> Option<D> {
        self.plugin_data
            .remove(&std::any::TypeId::of::<D>())
            .and_then(|d| d.downcast::<D>().ok())
            .map(|b| *b)
    }

    /// Add a serve hook that runs when the server starts.
    ///
    /// The hook receives a [`ServeContext`](crate::builder::ServeContext):
    /// the shared task registry (drain the tasks the hook owns via
    /// `take_of::<Tag>()`, or `take_all()` for single-consumer subsystems),
    /// the app shutdown token, and a `track()` collector for spawned tasks
    /// whose drain must be awaited at shutdown.
    pub fn on_serve<F>(&mut self, hook: F)
    where
        F: FnOnce(crate::builder::ServeContext) + Send + 'static,
    {
        self.serve_hooks.push(Box::new(hook));
    }

    /// Add a shutdown hook that runs when the server stops.
    pub fn on_shutdown<F>(&mut self, hook: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.shutdown_hooks.push(Box::new(hook));
    }

    /// Add an async shutdown hook that is awaited during server shutdown.
    ///
    /// Unlike [`on_shutdown`](Self::on_shutdown), the returned future is awaited
    /// as part of the shutdown sequence, so operations like graceful drain can
    /// actually complete within their configured timeout.
    pub fn on_shutdown_async<F, Fut>(&mut self, hook: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        self.async_shutdown_hooks
            .push(Box::new(move || Box::pin(hook())));
    }
}
