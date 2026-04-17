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
use crate::type_list::{PluginDeps, TAppend, TCons};
use std::any::Any;
use tokio_util::sync::CancellationToken;

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
pub trait Plugin: Send + 'static {
    /// Install this plugin into the given `AppBuilder`, returning the modified builder.
    fn install<T: Clone + Send + Sync + 'static>(self, app: AppBuilder<T>) -> AppBuilder<T>;

    /// Whether this plugin should be installed last in the layer stack.
    ///
    /// Plugins like `NormalizePath` need to be the outermost layer (installed last)
    /// to work correctly. When `should_be_last()` returns `true`, the builder will
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
/// fn install(self, (): (), ctx: &mut PluginInstallContext<'_>) -> MyConfig {
///     let name = ctx.config_get::<String>("my_plugin.name")
///         .unwrap_or_else(|| "default".into());
///     MyConfig { name }
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
///     type Provided = MyThing;
///     type Deps = (DbPool, CancellationToken);
///
///     fn install(self, (pool, token): (DbPool, CancellationToken), ctx: &mut PluginInstallContext<'_>) -> MyThing {
///         MyThing::new(pool, token)
///     }
/// }
/// ```
pub struct PluginInstallContext<'a> {
    deferred: Vec<DeferredAction>,
    config: Option<&'a crate::config::R2eConfig>,
}

impl<'a> PluginInstallContext<'a> {
    /// Create a new install context with access to config.
    pub(crate) fn new(config: Option<&'a crate::config::R2eConfig>) -> Self {
        Self {
            deferred: Vec::new(),
            config,
        }
    }

    /// Register a deferred action to run after state resolution.
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

    /// Consume the context and return the collected deferred actions.
    pub fn into_deferred(self) -> Vec<DeferredAction> {
        self.deferred
    }
}

/// A plugin that runs in the pre-state phase and provides a single bean.
///
/// This is the **simplified** plugin API — most plugins should implement this
/// trait. The `install` method receives resolved dependencies as a typed tuple
/// and a [`PluginInstallContext`] for registering deferred actions. No builder
/// generics, no `with_updated_types()`.
///
/// # Compile-Time Dependency Checking
///
/// Declare dependencies via [`Deps`](Self::Deps). The compiler verifies at
/// each `.plugin()` call site that all dependencies have already been provided:
///
/// ```ignore
/// AppBuilder::new()
///     .plugin(Scheduler)          // provides CancellationToken
///     .provide(pool)              // provides DbPool
///     .plugin(MyPlugin { .. })    // ✅ compiles: both deps are in P
///
/// // But swap the order:
/// AppBuilder::new()
///     .plugin(MyPlugin { .. })    // ❌ compile error: deps not yet provided
///     .plugin(Scheduler)
/// ```
///
/// For plugins that need to provide **multiple** beans or need full builder
/// access, implement [`RawPreStatePlugin`] instead.
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
///     type Provided = String;
///     type Deps = ();
///
///     fn install(self, (): (), _ctx: &mut PluginInstallContext<'_>) -> String {
///         self.value
///     }
/// }
/// ```
///
/// Plugin with dependencies:
///
/// ```ignore
/// impl PreStatePlugin for MyPlugin {
///     type Provided = MyService;
///     type Deps = (DbPool, CancellationToken);
///
///     fn install(self, (pool, token): (DbPool, CancellationToken), ctx: &mut PluginInstallContext<'_>) -> MyService {
///         MyService::new(pool, token)
///     }
/// }
/// ```
pub trait PreStatePlugin: Send + 'static {
    /// The bean type this plugin provides to the bean registry.
    type Provided: Clone + Send + Sync + 'static;

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
    /// `.with_bean::<T>()` registration. If a `with_bean`-registered type
    /// appears in `Deps`, runtime resolution panics with a clear message.
    type Deps: crate::type_list::PluginDeps;

    /// Install the plugin in the pre-state phase.
    ///
    /// `deps` contains the resolved dependency values declared by [`Deps`](Self::Deps).
    /// Return the value to be provided to the bean registry. Optionally
    /// register deferred actions via `ctx.add_deferred()`.
    fn install(self, deps: Self::Deps, ctx: &mut PluginInstallContext<'_>) -> Self::Provided;
}

/// A pre-state plugin with full builder access (advanced API).
///
/// Implement this trait when your plugin needs to:
/// - Provide **multiple** bean types (via `type Provisions = TCons<A, TCons<B, TNil>>`)
/// - Call arbitrary builder methods (`.with_bean()`, `.with_producer()`, etc.)
///
/// Most plugins should implement [`PreStatePlugin`] instead — it's simpler and
/// automatically provides a `RawPreStatePlugin` impl via a blanket implementation.
///
/// # Example
///
/// ```ignore
/// use r2e_core::{RawPreStatePlugin, AppBuilder, DeferredAction};
/// use r2e_core::builder::NoState;
/// use r2e_core::type_list::{TAppend, TCons, TNil};
/// use tokio_util::sync::CancellationToken;
///
/// pub struct Scheduler;
///
/// impl RawPreStatePlugin for Scheduler {
///     type Provisions = TCons<CancellationToken, TCons<JobRegistry, TNil>>;
///     type Required = TNil;
///
///     fn install<P, R>(self, app: AppBuilder<NoState, P, R>)
///         -> AppBuilder<NoState, <P as TAppend<Self::Provisions>>::Output, <R as TAppend<Self::Required>>::Output>
///     where
///         P: TAppend<Self::Provisions>,
///         R: TAppend<Self::Required>,
///     {
///         let token = CancellationToken::new();
///         let registry = JobRegistry::new();
///         app.provide(token)
///             .provide(registry)
///             .add_deferred(DeferredAction::new("Scheduler", |ctx| { /* ... */ }))
///             .with_updated_types()
///     }
/// }
/// ```
///
/// # `Required = TNil` and `with_updated_types()`
///
/// When `Required` is `TNil`, the compiler cannot prove that
/// `<R as TAppend<TNil>>::Output == R`. Since `R` is a phantom type parameter,
/// call [`.with_updated_types()`](AppBuilder::with_updated_types) at the end of
/// `install()` to perform the zero-cost phantom type conversion.
pub trait RawPreStatePlugin: Send + 'static {
    /// The type-level list of bean types this plugin provides.
    ///
    /// For a single provision: `TCons<MyType, TNil>`.
    /// For multiple: `TCons<A, TCons<B, TNil>>`.
    type Provisions;

    /// Bean dependencies this plugin requires from the bean graph.
    type Required;

    /// Install the plugin in the pre-state phase with full builder access.
    fn install<P, R>(
        self,
        app: AppBuilder<NoState, P, R>,
    ) -> AppBuilder<NoState, <P as TAppend<Self::Provisions>>::Output, <R as TAppend<Self::Required>>::Output>
    where
        P: TAppend<Self::Provisions>,
        R: TAppend<Self::Required>;
}

// Blanket impl: every PreStatePlugin is automatically a RawPreStatePlugin.
impl<T: PreStatePlugin> RawPreStatePlugin for T {
    type Provisions = TCons<T::Provided, crate::type_list::TNil>;
    type Required = <T::Deps as crate::type_list::PluginDeps>::AsList;

    fn install<P, R>(
        self,
        app: AppBuilder<NoState, P, R>,
    ) -> AppBuilder<NoState, <P as TAppend<Self::Provisions>>::Output, <R as TAppend<Self::Required>>::Output>
    where
        P: TAppend<Self::Provisions>,
        R: TAppend<Self::Required>,
    {
        let deps = T::Deps::resolve(app.bean_registry());
        let (provided, deferred) = {
            let mut ctx = PluginInstallContext::new(app.r2e_config_ref());
            let provided = PreStatePlugin::install(self, deps, &mut ctx);
            (provided, ctx.into_deferred())
        };
        let mut builder = app;
        for action in deferred {
            builder = builder.add_deferred(action);
        }
        builder.provide(provided).with_updated_types()
    }
}

// ── Deferred action system ─────────────────────────────────────────────────

/// A deferred action that runs after state resolution.
///
/// This is the mechanism for plugins that need to run setup code after
/// `build_state()` is called. Each action is a closure that receives a
/// `DeferredContext` providing access to builder internals.
///
/// # Example
///
/// ```ignore
/// impl PreStatePlugin for MyPlugin {
///     type Provided = MyToken;
///     type Deps = ();
///
///     fn install(self, (): (), ctx: &mut PluginInstallContext<'_>) -> MyToken {
///         let token = MyToken::new();
///         let handle = MyHandle::new(token.clone());
///
///         ctx.add_deferred(DeferredAction::new("MyPlugin", move |dctx| {
///             dctx.add_layer(Box::new(move |router| router.layer(Extension(handle))));
///             dctx.on_shutdown(|| { /* cleanup */ });
///         }));
///         token
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

/// Context for executing a deferred action.
///
/// Provides access to builder internals that deferred actions may need to modify.
pub struct DeferredContext<'a> {
    /// Layers to apply to the router.
    #[doc(hidden)]
    pub layers: &'a mut Vec<Box<dyn FnOnce(crate::http::Router) -> crate::http::Router + Send>>,
    /// Plugin data storage.
    #[doc(hidden)]
    pub plugin_data: &'a mut std::collections::HashMap<std::any::TypeId, Box<dyn Any + Send + Sync>>,
    /// Serve hooks (called when server starts). Each hook receives a clone
    /// of the shared `TaskRegistryHandle` and drains the tasks it owns.
    #[doc(hidden)]
    pub serve_hooks: &'a mut Vec<Box<dyn FnOnce(crate::builder::TaskRegistryHandle, CancellationToken) + Send>>,
    /// Shutdown hooks from plugins.
    #[doc(hidden)]
    pub shutdown_hooks: &'a mut Vec<Box<dyn FnOnce() + Send>>,
}

impl DeferredContext<'_> {
    /// Add a layer to the router.
    pub fn add_layer(&mut self, layer: Box<dyn FnOnce(crate::http::Router) -> crate::http::Router + Send>) {
        self.layers.push(layer);
    }

    /// Store plugin-specific data for later retrieval.
    ///
    /// Plugins can store arbitrary data keyed by type. This data persists
    /// through the builder lifecycle and can be retrieved during controller
    /// registration or serve hooks.
    pub fn store_data<D: Any + Send + Sync + 'static>(&mut self, data: D) {
        self.plugin_data.insert(std::any::TypeId::of::<D>(), Box::new(data));
    }

    /// Add a serve hook that runs when the server starts.
    ///
    /// The hook receives:
    /// - `registry`: Shared handle to the task registry; the hook drains the
    ///   tasks it owns via `registry.take_of::<Tag>()` (or `take_all()` for
    ///   single-consumer subsystems).
    /// - `token`: A cancellation token (unused by the builder, but passed for consistency)
    pub fn on_serve<F>(&mut self, hook: F)
    where
        F: FnOnce(crate::builder::TaskRegistryHandle, CancellationToken) + Send + 'static,
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
}

