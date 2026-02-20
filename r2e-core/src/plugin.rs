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
use crate::type_list::{TAppend, TCons};
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

/// Context passed to [`PreStatePlugin::install`] for registering deferred actions.
///
/// This is the simplified plugin API. Instead of receiving the full `AppBuilder`,
/// plugins create their provided value and optionally register deferred actions
/// (layers, serve/shutdown hooks) via this context.
///
/// # Example
///
/// ```ignore
/// impl PreStatePlugin for MyPlugin {
///     type Provided = MyConfig;
///     type Required = TNil;
///
///     fn install(self, ctx: &mut PluginInstallContext) -> MyConfig {
///         ctx.add_deferred(DeferredAction::new("MyPlugin", |dctx| {
///             dctx.on_shutdown(|| { tracing::info!("Bye"); });
///         }));
///         MyConfig { /* ... */ }
///     }
/// }
/// ```
pub struct PluginInstallContext {
    deferred: Vec<DeferredAction>,
}

impl PluginInstallContext {
    /// Create a new empty install context.
    pub fn new() -> Self {
        Self {
            deferred: Vec::new(),
        }
    }

    /// Register a deferred action to run after state resolution.
    pub fn add_deferred(&mut self, action: DeferredAction) {
        self.deferred.push(action);
    }

    /// Consume the context and return the collected deferred actions.
    pub fn into_deferred(self) -> Vec<DeferredAction> {
        self.deferred
    }
}

impl Default for PluginInstallContext {
    fn default() -> Self {
        Self::new()
    }
}

/// A plugin that runs in the pre-state phase and provides a single bean.
///
/// This is the **simplified** plugin API — most plugins should implement this
/// trait. The `install` method receives a [`PluginInstallContext`] for registering
/// deferred actions and returns the provided bean value directly. No builder
/// generics, no `with_updated_types()`.
///
/// For plugins that need to provide **multiple** beans or need full builder
/// access, implement [`RawPreStatePlugin`] instead.
///
/// Every `PreStatePlugin` is automatically a [`RawPreStatePlugin`] via a blanket
/// impl, so both work with `.plugin()`.
///
/// # Example
///
/// ```ignore
/// use r2e_core::{PreStatePlugin, PluginInstallContext, DeferredAction};
/// use r2e_core::type_list::TNil;
///
/// pub struct MyPlugin { pub value: String }
///
/// impl PreStatePlugin for MyPlugin {
///     type Provided = String;
///     type Required = TNil;
///
///     fn install(self, _ctx: &mut PluginInstallContext) -> String {
///         self.value
///     }
/// }
/// ```
pub trait PreStatePlugin: Send + 'static {
    /// The bean type this plugin provides to the bean registry.
    type Provided: Clone + Send + Sync + 'static;

    /// Bean dependencies this plugin requires from the bean graph.
    ///
    /// Most plugins set this to `TNil` (no additional requirements).
    type Required;

    /// Install the plugin in the pre-state phase.
    ///
    /// Return the value to be provided to the bean registry. Optionally
    /// register deferred actions via `ctx.add_deferred()`.
    fn install(self, ctx: &mut PluginInstallContext) -> Self::Provided;
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
    type Required = T::Required;

    fn install<P, R>(
        self,
        app: AppBuilder<NoState, P, R>,
    ) -> AppBuilder<NoState, <P as TAppend<Self::Provisions>>::Output, <R as TAppend<Self::Required>>::Output>
    where
        P: TAppend<Self::Provisions>,
        R: TAppend<Self::Required>,
    {
        let mut ctx = PluginInstallContext::new();
        let provided = PreStatePlugin::install(self, &mut ctx);
        let mut builder = app;
        for action in ctx.into_deferred() {
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
///     type Required = TNil;
///
///     fn install<P, R>(self, app: AppBuilder<NoState, P, R>) -> AppBuilder<NoState, TCons<Self::Provided, P>, <R as TAppend<Self::Required>>::Output>
///     where
///         R: TAppend<Self::Required>,
///     {
///         let token = MyToken::new();
///         let handle = MyHandle::new(token.clone());
///
///         app.provide(token).add_deferred(DeferredAction::new("MyPlugin", move |ctx| {
///             ctx.add_layer(Box::new(move |router| router.layer(Extension(handle))));
///             ctx.on_shutdown(|| { /* cleanup */ });
///         })).with_updated_types()
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
    /// Serve hooks (called when server starts).
    #[doc(hidden)]
    pub serve_hooks: &'a mut Vec<Box<dyn FnOnce(Vec<Box<dyn Any + Send>>, CancellationToken) + Send>>,
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
    /// - `tasks`: Type-erased task definitions collected during controller registration
    /// - `token`: A cancellation token (unused by the builder, but passed for consistency)
    pub fn on_serve<F>(&mut self, hook: F)
    where
        F: FnOnce(Vec<Box<dyn Any + Send>>, CancellationToken) + Send + 'static,
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

// ── Legacy types (deprecated) ──────────────────────────────────────────────

/// A type-erased deferred plugin that can be installed after state resolution.
///
/// # Deprecated
///
/// Use [`DeferredAction`] instead.
#[deprecated(since = "0.2.0", note = "Use DeferredAction instead")]
#[allow(deprecated)]
pub struct DeferredPlugin {
    /// The plugin's setup data, type-erased.
    pub data: Box<dyn Any + Send>,
    /// The installer function.
    pub installer: Box<dyn DeferredPluginInstaller>,
}

#[allow(deprecated)]
impl DeferredPlugin {
    /// Create a new deferred plugin.
    pub fn new<D: Send + 'static, I: DeferredPluginInstaller + 'static>(
        data: D,
        installer: I,
    ) -> Self {
        Self {
            data: Box::new(data),
            installer: Box::new(installer),
        }
    }
}

/// Trait for installing a deferred plugin into a typed builder.
///
/// # Deprecated
///
/// Use [`DeferredAction`] instead.
#[deprecated(since = "0.2.0", note = "Use DeferredAction instead")]
#[allow(deprecated)]
pub trait DeferredPluginInstaller: Send {
    /// Install the plugin using the provided data and context.
    fn install(
        &self,
        data: Box<dyn Any + Send>,
        ctx: &mut dyn DeferredInstallContext,
    );
}

/// Context for installing a deferred plugin.
///
/// # Deprecated
///
/// Use [`DeferredContext`] instead.
#[deprecated(since = "0.2.0", note = "Use DeferredContext instead")]
pub trait DeferredInstallContext {
    /// Add a layer to the router.
    fn add_layer(&mut self, layer: Box<dyn FnOnce(crate::http::Router) -> crate::http::Router + Send>);

    /// Store plugin-specific data for later retrieval.
    fn store_plugin_data(&mut self, data: Box<dyn Any + Send + Sync>);

    /// Add a serve hook that runs when the server starts.
    fn add_serve_hook(
        &mut self,
        hook: Box<dyn FnOnce(Vec<Box<dyn Any + Send>>, CancellationToken) + Send>,
    );

    /// Add a shutdown hook that runs when the server stops.
    fn add_shutdown_hook(&mut self, hook: Box<dyn FnOnce() + Send>);
}
