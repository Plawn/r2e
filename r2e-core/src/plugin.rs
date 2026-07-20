//! Plugin system for R2E.
//!
//! Plugins are composable units of functionality that can be installed into an
//! [`AppBuilder`] using the `.with(plugin)` method.
//!
//! # Two plugin traits
//!
//! - [`Plugin`]: For plugins that don't provide beans (most common). Works in
//!   the post-state phase, after `build_state()`.
//! - [`PreStatePlugin`]: For plugins that provide beans (like Scheduler).
//!   Works in the pre-state phase, before `build_state()`.
//!
//! Both traits use the same `.with(plugin)` method on `AppBuilder`.

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

/// Context passed to [`PreStatePlugin::install`] for registering deferred actions
/// and accessing configuration.
///
/// This is the simplified plugin API. Instead of receiving the full `AppBuilder`,
/// plugins create their provided value and optionally register deferred actions
/// (layers, serve/shutdown hooks) via this context.
///
/// # Configuration Access
///
/// Plugins can read configuration values loaded by [`AppBuilder::load_config`]:
///
/// ```ignore
/// fn install(self, (): (), ctx: &mut PluginInstallContext<'_>) -> (MyConfig,) {
///     let name = ctx.config_get::<String>("my_plugin.name")
///         .unwrap_or_else(|| "default".into());
///     (MyConfig { name },)
/// }
/// ```
///
/// # Typed Dependencies
///
/// For bean dependencies, declare [`PreStatePlugin::Deps`] instead of reading
/// from the context. Dependencies are passed as a typed tuple parameter and
/// verified at compile time:
///
/// ```ignore
/// impl PreStatePlugin for MyPlugin {
///     type Provided = (MyThing,);
///     type Deps = (DbPool, CancellationToken);
///     type LateDeps = ();
///
///     fn install(self, (pool, token): (DbPool, CancellationToken), ctx: &mut PluginInstallContext<'_>) -> (MyThing,) {
///         (MyThing::new(pool, token),)
///     }
/// }
/// ```
pub struct PluginInstallContext<'a> {
    deferred: Vec<DeferredAction>,
    /// Buffered sugar calls ([`add_layer`](Self::add_layer),
    /// [`on_serve`](Self::on_serve), etc.). Flushed as ONE [`DeferredAction`]
    /// by the blanket `RawPreStatePlugin` impl — see [`flush`](Self::flush).
    sugar: Vec<Box<dyn FnOnce(&mut DeferredContext) + Send>>,
    /// Lifecycle-hook registrars applied to the bean registry right after the
    /// plugin's `Provided` values are deposited. Backs
    /// [`run_post_construct`](Self::run_post_construct) /
    /// [`run_pre_destroy`](Self::run_pre_destroy).
    registry_ops: Vec<Box<dyn FnOnce(&mut crate::beans::BeanRegistry) + Send>>,
    config: Option<&'a crate::config::R2eConfig>,
}

impl<'a> PluginInstallContext<'a> {
    /// Create a new install context with access to config.
    pub(crate) fn new(config: Option<&'a crate::config::R2eConfig>) -> Self {
        Self {
            deferred: Vec::new(),
            sugar: Vec::new(),
            registry_ops: Vec::new(),
            config,
        }
    }

    /// Run a [`PostConstruct`](crate::PostConstruct) hook for one of this
    /// plugin's `Provided` beans once the graph is resolved.
    ///
    /// The plugin's `Provided` values are second-class lifecycle citizens by
    /// default (deposited straight into the graph). Call this in `install` to
    /// opt a provided type `B` into the same post-construct lifecycle as a
    /// factory bean: the hook fires during `build_state()`, after every
    /// factory-bean post-construct, reading `B` from the resolved graph.
    pub fn run_post_construct<B: crate::PostConstruct + Clone>(&mut self) {
        self.registry_ops
            .push(Box::new(|reg| reg.register_provided_post_construct::<B>()));
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
    /// order**: `[A.explicit…, A.sugar, A.configure, B.explicit…, B.sugar,
    /// B.configure]`. In particular, a layer added from plugin A's
    /// [`configure`](crate::PreStatePlugin::configure) is applied *before*
    /// (i.e. nested inside) a layer added at install time by a
    /// later-installed plugin B — there is no "all installs, then all
    /// configures" phase separation.
    pub fn add_deferred(&mut self, action: DeferredAction) {
        self.deferred.push(action);
    }

    /// Returns the loaded [`R2eConfig`], if available.
    ///
    /// This is `Some` when [`AppBuilder::load_config`] or [`AppBuilder::with_config`]
    /// was called before this plugin was installed.
    pub fn config(&self) -> Option<&crate::config::R2eConfig> {
        self.config
    }

    /// Read a typed configuration value by key.
    ///
    /// Shorthand for `ctx.config().and_then(|c| c.get::<T>(key).ok())`.
    pub fn config_get<T: crate::config::FromConfigValue>(&self, key: &str) -> Option<T> {
        self.config.and_then(|c| c.get::<T>(key).ok())
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
        let PluginInstallContext {
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
/// This is the **simplified** plugin API — most plugins should implement this
/// trait. The `install` method receives resolved dependencies as a typed tuple
/// and a [`PluginInstallContext`] for registering deferred actions. No builder
/// generics, no `with_updated_types()`.
///
/// # Two-stage lifecycle
///
/// A pre-state plugin participates in two phases of app assembly:
///
/// 1. **install** — runs at `.plugin()` time, *before* the bean graph is built.
///    It produces the plugin's [`Provided`](Self::Provided) beans and may
///    register deferred actions. Its [`Deps`](Self::Deps) are resolved here, so
///    they can only name beans already supplied via `.provide(instance)`.
/// 2. **configure** — runs *after* [`build_state()`](crate::AppBuilder::build_state),
///    with the fully materialized bean graph in hand. Its
///    [`LateDeps`](Self::LateDeps) can name **any** bean — `.provide()`-d,
///    `.register()`-ed (factory-built), or produced by another plugin.
///
/// ```text
///   .plugin(Me)              build_state()             (serve)
///        │                        │                       │
///        ▼                        ▼                       ▼
///     install(Deps)  ─────►  [bean graph built]  ─►  configure(LateDeps)
/// ```
///
/// ## `Deps` vs `LateDeps`
///
/// - **`Deps`** = pre-built infrastructure you hand to `.provide()` (a
///   `DbPool`, a `CancellationToken`). Available at install time.
/// - **`LateDeps`** = anything else, including **factory-built beans**
///   (`.register::<T>()`) and beans other plugins provide. Available only in
///   `configure()`.
///
/// Rule of thumb: if the type is `.provide()`-d, put it in `Deps`; otherwise
/// put it in `LateDeps` and consume it from `configure()`.
///
/// [`Provided`](Self::Provided) is a **tuple** of beans: `(A,)` for a single
/// bean, `(A, B)` for several, or `()` for none. This covers multi-bean plugins
/// too — there is no longer any need to drop down to [`RawPreStatePlugin`] just
/// to provide more than one bean.
///
/// # Compile-Time Dependency Checking
///
/// Declare dependencies via [`Deps`](Self::Deps). The compiler verifies at
/// each `.plugin()` call site that all dependencies have already been provided:
///
/// ```ignore
/// AppBuilder::new()
///     .plugin(Scheduler)          // provides CancellationToken
///     .plugin(Executor)           // Scheduler runs ticks on the shared pool
///     .provide(pool)              // provides DbPool
///     .plugin(MyPlugin { .. })    // ✅ compiles: both deps are in P
///
/// // But swap the order:
/// AppBuilder::new()
///     .plugin(MyPlugin { .. })    // ❌ compile error: deps not yet provided
///     .plugin(Scheduler)
/// ```
///
/// For plugins that need arbitrary builder access (calling `.register()`,
/// `.provide()`, etc. by hand), implement [`RawPreStatePlugin`] instead — but
/// that is rarely necessary.
///
/// Every `PreStatePlugin` is automatically a [`RawPreStatePlugin`] via a blanket
/// impl, so both work with `.plugin()`.
///
/// # Examples
///
/// Simple plugin (no dependencies):
///
/// ```ignore
/// use r2e_core::{PreStatePlugin, PluginInstallContext};
///
/// pub struct MyPlugin { pub value: String }
///
/// impl PreStatePlugin for MyPlugin {
///     type Provided = (String,);
///     type Deps = ();
///     type LateDeps = ();
///
///     fn install(self, (): (), _ctx: &mut PluginInstallContext<'_>) -> (String,) {
///         (self.value,)
///     }
/// }
/// ```
///
/// Plugin with dependencies:
///
/// ```ignore
/// impl PreStatePlugin for MyPlugin {
///     type Provided = (MyService,);
///     type Deps = (DbPool, CancellationToken);
///     type LateDeps = ();
///
///     fn install(self, (pool, token): (DbPool, CancellationToken), ctx: &mut PluginInstallContext<'_>) -> (MyService,) {
///         (MyService::new(pool, token),)
///     }
/// }
/// ```
///
/// Multi-bean plugin:
///
/// ```ignore
/// impl PreStatePlugin for Scheduler {
///     type Provided = (CancellationToken, ScheduledJobRegistry);
///     type Deps = ();
///     type LateDeps = ();
///
///     fn install(self, (): (), ctx: &mut PluginInstallContext<'_>) -> (CancellationToken, ScheduledJobRegistry) {
///         let token = CancellationToken::new();
///         let registry = ScheduledJobRegistry::new();
///         // ... ctx.add_layer(..) / ctx.on_serve(..) / ctx.on_shutdown(..) ...
///         (token, registry)
///     }
/// }
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not implement a pre-state plugin trait (`PreStatePlugin`/`RawPreStatePlugin`), the API used by `.plugin()`",
    label = "`.plugin()` needs a pre-state plugin",
    note = "if `{Self}` is a post-state plugin, install it with `.with({Self})` AFTER `build_state()` instead of `.plugin({Self})`"
)]
pub trait PreStatePlugin: Send + 'static {
    /// The **tuple** of bean types this plugin provides to the bean registry.
    ///
    /// Use `(A,)` for a single bean, `(A, B)` for several, or `()` for none.
    /// Each element must be `Clone + Send + Sync + 'static`.
    type Provided: PluginProvisions;

    /// Bean dependencies this plugin requires, as a concrete tuple.
    ///
    /// The compiler checks at each `.plugin()` call site that every type in
    /// this tuple has already been provided (via `.provide()` or an earlier
    /// `.plugin()`). Dependencies are resolved from the bean registry and
    /// passed to [`install()`](Self::install) as the `deps` parameter.
    ///
    /// Most plugins set this to `()` (no dependencies).
    ///
    /// **Constraint:** plugin install runs *before* the bean graph is built,
    /// so every type listed here must be a `.provide(instance)` value, not a
    /// `.register::<T>()` registration. If a `register`-ed (factory-built) type
    /// appears in `Deps`, runtime resolution panics with a message steering you
    /// to move it to [`LateDeps`](Self::LateDeps). For beans that are not
    /// `.provide()`-d, use `LateDeps` + [`configure`](Self::configure) instead.
    type Deps: crate::type_list::PluginDeps;

    /// Bean dependencies resolved **after** `build_state()`, from the fully
    /// materialized bean graph.
    ///
    /// Unlike [`Deps`](Self::Deps), these may name any bean — including
    /// `.register::<T>()`-ed (factory-built) beans and beans other plugins
    /// provide — because they are resolved in [`configure`](Self::configure),
    /// after the whole graph is constructed. They are appended to the builder's
    /// requirement list and verified against the **final** provision list at
    /// `build_state()` (not at the `.plugin()` call site), so a dependency may
    /// be `.register()`-ed *after* this plugin is installed.
    ///
    /// Most plugins set this to `()` (no late dependencies). On stable Rust
    /// associated types have no defaults, so every impl must write it
    /// explicitly:
    ///
    /// ```ignore
    /// type LateDeps = ();
    /// ```
    type LateDeps: crate::type_list::PluginDeps;

    /// The plugin's typed configuration section.
    ///
    /// Set to `()` (the common case) for a plugin that reads no typed config —
    /// it can still reach for the stringly [`PluginInstallContext::config_get`]
    /// escape hatch. For typed config, set this to any
    /// `#[derive(ConfigProperties)]` struct and point [`CONFIG_PREFIX`](Self::CONFIG_PREFIX)
    /// at its YAML section. The framework then loads and validates that section
    /// after `build_state()` and hands it to [`configure`](Self::configure) — a
    /// malformed value produces the same boot error a controller's
    /// `#[config(section)]` mismatch does.
    ///
    /// On stable Rust associated types have no defaults, so every impl must
    /// write it explicitly (`type Config = ();`).
    type Config: crate::config::PluginConfig;

    /// The YAML section prefix for [`Config`](Self::Config).
    ///
    /// `None` (the default) disables typed-config loading — use it with
    /// `type Config = ();`. `Some("prometheus")` loads the `Config` from the
    /// `prometheus.*` section. The section is treated as **optional**
    /// (presence-based, like a controller's `Option<Section>`): if no config was
    /// loaded, or no key lives under the prefix, [`configure`](Self::configure)
    /// receives `None`. A present-but-invalid section is a boot error.
    const CONFIG_PREFIX: Option<&'static str> = None;

    /// Install the plugin in the pre-state phase.
    ///
    /// `deps` contains the resolved dependency values declared by [`Deps`](Self::Deps).
    /// Return the value to be provided to the bean registry. Optionally
    /// register deferred actions via `ctx.add_deferred()`.
    ///
    /// Takes `&mut self` (not `self`) so the plugin instance survives into the
    /// post-state [`configure`](Self::configure) call, where the loaded config
    /// is available to merge with programmatic builder settings. Move owned
    /// fields out with [`std::mem::take`] / clone if `install` needs them, or
    /// leave them for `configure` (which receives `self` by value).
    fn install(&mut self, deps: Self::Deps, ctx: &mut PluginInstallContext<'_>) -> Self::Provided;

    /// Configure the plugin in the **post-state** phase, after `build_state()`.
    ///
    /// Called once — consuming the plugin instance (`self`) so it can merge its
    /// programmatic builder settings with file config — with the plugin's
    /// [`Provided`](Self::Provided) beans (as a borrowed copy), its resolved
    /// [`LateDeps`](Self::LateDeps), the loaded typed [`Config`](Self::Config)
    /// (see below), and a [`DeferredContext`] for adding layers, serve/shutdown
    /// hooks, or plugin data — exactly the same surface deferred actions get.
    /// Use this to wire up anything that needs a factory-built or app-level bean.
    ///
    /// The default is a no-op, so plugins with `type LateDeps = ()`,
    /// `type Config = ()`, and no post-state work need not implement it.
    ///
    /// # `config`
    ///
    /// `config` is `Some(cfg)` only when [`CONFIG_PREFIX`](Self::CONFIG_PREFIX)
    /// is `Some(prefix)`, config was loaded, and a key lives under that prefix;
    /// otherwise it is `None` (see the presence rules on
    /// [`CONFIG_PREFIX`](Self::CONFIG_PREFIX)). When the section is present but
    /// malformed, the framework panics with a controller-grade validation error
    /// **before** calling `configure`. Precedence for a config-consuming plugin
    /// is: explicit builder setting (a field on `self`) > `config` (file) >
    /// built-in default.
    ///
    /// # `provided` and pinned overrides
    ///
    /// `provided` is the plugin's **own** instance — a copy of exactly what
    /// [`install`](Self::install) returned. If a test harness pin-overrides one
    /// of the `Provided` types (e.g. `override_bean(mock)` /
    /// `BeanRegistry::pin_provide`), the state and bean context hold the
    /// override, but `provided` still holds the plugin's original value: the
    /// plugin owns what it built. To observe the bean **as the rest of the app
    /// sees it** (override included), read it through the graph instead — list
    /// it in [`LateDeps`](Self::LateDeps) or use `ctx.bean_context()`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// impl PreStatePlugin for MetricsExporter {
    ///     type Provided = (ExporterHandle,);
    ///     type Deps = ();
    ///     type LateDeps = (MetricsRegistry,); // registered elsewhere via `.register()`
    ///     type Config = ();
    ///
    ///     fn install(&mut self, (): (), _ctx: &mut PluginInstallContext<'_>) -> (ExporterHandle,) {
    ///         (ExporterHandle::new(),)
    ///     }
    ///
    ///     fn configure(
    ///         self,
    ///         (handle,): &(ExporterHandle,),
    ///         (registry,): (MetricsRegistry,),
    ///         _config: Option<()>,
    ///         ctx: &mut DeferredContext<'_>,
    ///     ) {
    ///         let handle = handle.clone();
    ///         ctx.on_serve(move |_sc| handle.bind(registry));
    ///     }
    /// }
    /// ```
    #[allow(unused_variables)]
    fn configure(
        self,
        provided: &Self::Provided,
        deps: Self::LateDeps,
        config: Option<Self::Config>,
        ctx: &mut DeferredContext<'_>,
    ) where
        Self: Sized,
    {
    }
}

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

    /// Bean dependencies this plugin requires from the bean graph, checked at
    /// the `.plugin()` **call site** against the provisions present so far.
    ///
    /// This is the pre-state `Deps` list: it must already be provided when the
    /// plugin is installed. It is a *sublist* of [`AllRequired`](Self::AllRequired)
    /// (the part that gets a call-site check).
    type Required;

    /// The full requirement list appended to the builder's `R`: the pre-state
    /// `Deps` (`Required`) concatenated with the post-state `LateDeps`.
    ///
    /// Only [`Required`](Self::Required) is checked at the `.plugin()` call
    /// site; the `LateDeps` portion rides along in `R` and is verified against
    /// the **final** provision list at `build_state()`, so a `LateDeps` bean may
    /// be registered *after* this plugin is installed.
    type AllRequired;

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
        R: TAppend<Self::AllRequired>;
}

// Blanket impl: every PreStatePlugin is automatically a RawPreStatePlugin.
//
// The plugin's `Provided` tuple maps to the type-level provision list via
// `PluginProvisions::AsList`, and its values are deposited into the bean
// registry with a single `provide_all` (value-level insertion only). The
// type-level list is then advanced in one phantom `with_updated_types()` cast —
// this keeps override/pinning/ordering semantics identical to calling
// `.provide()` per bean, which matters for `TestApp` bean overrides.
impl<T> RawPreStatePlugin for T
where
    T: PreStatePlugin,
    // `AsList` is always a `TCons`/`TNil` chain, so this always holds — but it
    // must be stated for the `AllRequired` associated type below to be
    // well-formed for an abstract `T`.
    <T::Deps as PluginDeps>::AsList: TAppend<<T::LateDeps as PluginDeps>::AsList>,
{
    type Provisions = <T::Provided as PluginProvisions>::AsList;
    type Required = <T::Deps as PluginDeps>::AsList;
    type AllRequired =
        <<T::Deps as PluginDeps>::AsList as TAppend<<T::LateDeps as PluginDeps>::AsList>>::Output;

    fn install<P, R, Mods>(
        self,
        app: AppBuilder<NoState, P, R, Mods>,
    ) -> crate::builder::WithPluginInstalled<Self, P, R, Mods>
    where
        P: TAppend<Self::Provisions>,
        R: TAppend<Self::AllRequired>,
    {
        let deps = T::Deps::resolve(app.bean_registry());
        let name = plugin_action_name::<T>();
        // The `<prefix>.enabled` config gate (phase 6). When a plugin declares a
        // `CONFIG_PREFIX` and `<prefix>.enabled` is `false`, its POST-STATE
        // effects are skipped: the buffered sugar action, any explicit
        // `add_deferred` actions, and `configure`. Beans it `Provided` still
        // exist in the graph (the type-level provision list is fixed at compile
        // time — disabling a plugin never removes its beans), and its
        // lifecycle-hook registrars (`run_post_construct`/`run_pre_destroy`) still
        // run, since those beans are real and may be injected by other code.
        let prefix = T::CONFIG_PREFIX;
        // Keep the plugin instance alive past `install` so `configure` can move
        // it in by value (and merge its programmatic fields with file config).
        let mut plugin = self;
        let (provided, registry_ops, deferred) = {
            let mut ctx = PluginInstallContext::new(app.r2e_config_ref());
            // Fully qualify: `install`/`configure` exist on both this trait and
            // `RawPreStatePlugin`, so `plugin.install(..)` would be ambiguous.
            let provided = PreStatePlugin::install(&mut plugin, deps, &mut ctx);
            // Lift the lifecycle registrars (run_post_construct / run_pre_destroy)
            // out before `flush` consumes the context.
            let registry_ops = ctx.take_registry_ops();
            (provided, registry_ops, ctx.flush(name))
        };
        let mut builder = app;
        for action in deferred {
            // Gate every buffered/explicit post-state action on `<prefix>.enabled`.
            builder = builder.add_deferred(gate_on_enabled(action, prefix));
        }
        // Keep a copy of the provided beans for the post-state `configure`
        // call — `provide_all` consumes the original into the registry.
        let provided_for_configure = provided.clone_all();
        provided.provide_all(builder.bean_registry_mut());
        // Apply any post-construct / pre-destroy registrations the plugin opted
        // its `Provided` beans into — after the values are deposited. NOT gated
        // by `enabled`: the beans still exist, so their lifecycle stays honest.
        for op in registry_ops {
            op(builder.bean_registry_mut());
        }
        // Schedule `configure` to run after `build_state()`, when the full bean
        // graph is available to resolve `LateDeps` from AND `R2eConfig` is
        // guaranteed loaded (so typed `Config` is delivered here, not at
        // install). Runs as a deferred action (post-resolution). For
        // `LateDeps = ()`, `Config = ()`, and the default (no-op) `configure`,
        // this is an inert closure. Also gated on `<prefix>.enabled`; this action
        // is the single place we emit the "plugin disabled" diagnostic, since
        // exactly one configure action is scheduled per plugin.
        builder = builder.add_deferred(DeferredAction::new(name, move |dctx| {
            if !plugin_config_enabled(dctx.config(), prefix) {
                tracing::info!(
                    plugin = name,
                    "plugin disabled via `{}.enabled = false`; post-state effects skipped (its beans remain in the graph)",
                    prefix.unwrap_or(name),
                );
                return;
            }
            let late = <T::LateDeps as PluginDeps>::resolve_from_context(dctx.bean_context());
            let config = load_plugin_config::<T>(dctx.config(), name);
            PreStatePlugin::configure(plugin, &provided_for_configure, late, config, dctx);
        }));
        builder.with_updated_types()
    }
}

/// Load and validate a plugin's typed [`Config`](PreStatePlugin::Config) section
/// at `configure` time, when [`R2eConfig`](crate::config::R2eConfig) is
/// guaranteed loaded.
///
/// The section is optional (presence-based, like a controller's
/// `Option<Section>`): returns `None` when config loading is disabled
/// (`CONFIG_PREFIX == None`), no config was loaded, or no key lives under the
/// prefix. A present-but-invalid section panics with the same validation report
/// a controller `#[config]` mismatch produces (`plugin` names the plugin in the
/// message).
fn load_plugin_config<T: PreStatePlugin>(
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
/// the `configure` action instead (see the blanket `install`).
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
/// Most plugins never construct one directly: the sugar methods on
/// [`PluginInstallContext`] ([`add_layer`](PluginInstallContext::add_layer),
/// [`on_shutdown`](PluginInstallContext::on_shutdown), …) buffer plain closures
/// and are flushed into a single `DeferredAction` automatically. Reach for
/// `add_deferred(DeferredAction::new(..))` only as an escape hatch.
///
/// # Example (preferred — sugar)
///
/// ```ignore
/// impl PreStatePlugin for MyPlugin {
///     type Provided = (MyToken,);
///     type Deps = ();
///     type LateDeps = ();
///
///     fn install(self, (): (), ctx: &mut PluginInstallContext<'_>) -> (MyToken,) {
///         let token = MyToken::new();
///         let handle = MyHandle::new(token.clone());
///
///         ctx.add_layer(move |router| router.layer(Extension(handle)));
///         ctx.on_shutdown(|| { /* cleanup */ });
///         (token,)
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
    /// [`bean_context`](DeferredContext::bean_context) — this is what backs a
    /// plugin's post-state [`configure`](crate::PreStatePlugin::configure)
    /// `LateDeps` resolution.
    #[doc(hidden)]
    pub bean_context: &'a crate::beans::BeanContext,
    /// The loaded [`R2eConfig`](crate::config::R2eConfig), if any. Deferred
    /// actions run inside `build_state()`, which always follows `load_config` /
    /// `with_config`, so this backs a plugin's post-state typed-`Config` loading
    /// (see [`configure`](crate::PreStatePlugin::configure)). `None` only when
    /// neither `load_config` nor `with_config` was called.
    #[doc(hidden)]
    pub config: Option<&'a crate::config::R2eConfig>,
}

impl DeferredContext<'_> {
    /// The fully resolved bean graph.
    ///
    /// Deferred actions run after `build_state()`, so every bean —
    /// `.provide()`-d, `.register()`-ed (factory-built), or produced by another
    /// plugin — is materialized and readable here (`ctx.bean_context().get::<T>()`).
    /// This is how a plugin's [`configure`](crate::PreStatePlugin::configure)
    /// hook resolves its `LateDeps`.
    pub fn bean_context(&self) -> &crate::beans::BeanContext {
        self.bean_context
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
    /// [`PluginInstallContext::store_data`]: a plugin can stash a non-`Clone`
    /// value at install time (buffered sugar is flushed into plugin data before
    /// `configure` runs) and move it out here in its
    /// [`configure`](crate::PreStatePlugin::configure) hook — e.g. a command
    /// channel receiver that must travel into a serve hook. Returns `None` when
    /// no value of type `D` was stored.
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
