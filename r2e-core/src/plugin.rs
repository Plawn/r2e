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
use crate::type_list::TCons;
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

// ── Pre-state Plugin trait ─────────────────────────────────────────────────

/// A plugin that runs in the pre-state phase and provides beans.
///
/// Pre-state plugins are installed before `build_state()` is called. They can:
/// - Provide bean instances to the bean registry
/// - Register deferred actions (like scheduler setup) that execute after state resolution
///
/// The `Provided` associated type specifies the bean type this plugin provides,
/// which becomes available for injection via `#[inject]`.
///
/// # Example
///
/// ```ignore
/// use r2e_core::{PreStatePlugin, DeferredAction};
/// use tokio_util::sync::CancellationToken;
///
/// pub struct Scheduler;
///
/// impl PreStatePlugin for Scheduler {
///     type Provided = CancellationToken;
///
///     fn install<P>(self, app: AppBuilder<NoState, P>) -> AppBuilder<NoState, TCons<Self::Provided, P>> {
///         let token = CancellationToken::new();
///         app.provide(token.clone()).add_deferred(DeferredAction::new("Scheduler", move |ctx| {
///             // ... setup ...
///         }))
///     }
/// }
/// ```
pub trait PreStatePlugin: Send + 'static {
    /// The type this plugin provides to the bean registry.
    type Provided: Clone + Send + Sync + 'static;

    /// Install the plugin in the pre-state phase.
    ///
    /// The implementation should:
    /// 1. Create the provided instance
    /// 2. Call `app.provide(instance)` to register it
    /// 3. Optionally call `app.add_deferred()` for post-state setup
    fn install<P>(self, app: AppBuilder<NoState, P>) -> AppBuilder<NoState, TCons<Self::Provided, P>>;
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
///
///     fn install<P>(self, app: AppBuilder<NoState, P>) -> AppBuilder<NoState, TCons<Self::Provided, P>> {
///         let token = MyToken::new();
///         let handle = MyHandle::new(token.clone());
///
///         app.provide(token).add_deferred(DeferredAction::new("MyPlugin", move |ctx| {
///             ctx.add_layer(Box::new(move |router| router.layer(Extension(handle))));
///             ctx.on_shutdown(|| { /* cleanup */ });
///         }))
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
    pub(crate) layers: &'a mut Vec<Box<dyn FnOnce(crate::http::Router) -> crate::http::Router + Send>>,
    /// Plugin data storage.
    pub(crate) plugin_data: &'a mut std::collections::HashMap<std::any::TypeId, Box<dyn Any + Send + Sync>>,
    /// Serve hooks (called when server starts).
    pub(crate) serve_hooks: &'a mut Vec<Box<dyn FnOnce(Vec<Box<dyn Any + Send>>, CancellationToken) + Send>>,
    /// Shutdown hooks from plugins.
    pub(crate) shutdown_hooks: &'a mut Vec<Box<dyn FnOnce() + Send>>,
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
