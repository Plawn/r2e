use crate::builder::{AppBuilder, NoState};
use crate::type_list::TCons;

/// A composable unit of functionality that can be installed into an [`AppBuilder`].
///
/// Plugins replace the old `with_cors()`, `with_tracing()`, etc. methods with a
/// single, uniform `.with(plugin)` entry point. This makes the builder extensible
/// without requiring new methods on `AppBuilder` for every cross-cutting concern.
///
/// # Built-in plugins
///
/// See [`crate::plugins`] for the plugins shipped with `quarlus-core`:
/// [`Cors`](crate::plugins::Cors), [`Tracing`](crate::plugins::Tracing),
/// [`Health`](crate::plugins::Health), [`ErrorHandling`](crate::plugins::ErrorHandling),
/// [`DevReload`](crate::plugins::DevReload).
///
/// # Example
///
/// ```ignore
/// use quarlus_core::plugins::{Cors, Tracing, Health, ErrorHandling, DevReload};
///
/// AppBuilder::new()
///     .build_state::<Services>()
///     .with(Health)
///     .with(Cors::permissive())
///     .with(Tracing)
///     .with(ErrorHandling)
///     .with(DevReload)
/// ```
pub trait Plugin<T: Clone + Send + Sync + 'static> {
    /// Install this plugin into the given `AppBuilder`, returning the modified builder.
    fn install(self, app: AppBuilder<T>) -> AppBuilder<T>;
}

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
/// use quarlus_scheduler::Scheduler;
///
/// AppBuilder::new()
///     .with_plugin(Scheduler)  // Provides CancellationToken, defers scheduler setup
///     .build_state::<Services, _>()
///     .register_controller::<MyController>()
///     .serve("0.0.0.0:3000")
/// ```
pub trait PreStatePlugin: Send + 'static {
    /// The type this plugin provides to the bean registry.
    type Provided: Clone + Send + Sync + 'static;

    /// Install the plugin in the pre-state phase.
    ///
    /// The implementation should:
    /// 1. Create the provided instance
    /// 2. Call `app.provide(instance)` to register it
    /// 3. Optionally call `app.add_deferred_plugin()` for post-state setup
    fn pre_install<P>(self, app: AppBuilder<NoState, P>) -> AppBuilder<NoState, TCons<Self::Provided, P>>;
}

use std::any::Any;

/// A type-erased deferred plugin that can be installed after state resolution.
///
/// This is the generic mechanism for plugins that need to run setup code after
/// `build_state()` is called. The plugin stores its data as `Box<dyn Any>` and
/// provides an installer that knows how to process that data.
///
/// # How it works
///
/// 1. In `pre_install()`, a plugin creates a `DeferredPlugin` containing:
///    - Its setup data (boxed as `Any`)
///    - An installer function that knows how to process that data
///
/// 2. The `DeferredPlugin` is stored in `BuilderConfig::deferred_plugins`
///
/// 3. When `from_pre<T>()` is called, each deferred plugin's installer is
///    invoked with a `DeferredInstallContext<T>` that provides access to
///    builder internals.
pub struct DeferredPlugin {
    /// The plugin's setup data, type-erased.
    pub data: Box<dyn Any + Send>,
    /// The installer function.
    pub installer: Box<dyn DeferredPluginInstaller>,
}

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
/// Implementors receive the plugin's data (as `Box<dyn Any>`) and a context
/// that provides access to builder internals. The installer can downcast
/// the data to its expected type and modify the builder accordingly.
pub trait DeferredPluginInstaller: Send {
    /// Install the plugin using the provided data and context.
    fn install(
        &self,
        data: Box<dyn Any + Send>,
        ctx: &mut dyn DeferredInstallContext,
    );
}

use tokio_util::sync::CancellationToken;

/// Context for installing a deferred plugin.
///
/// This trait provides type-erased access to builder internals that deferred
/// plugins may need to modify.
pub trait DeferredInstallContext {
    /// Add a layer to the router.
    fn add_layer(&mut self, layer: Box<dyn FnOnce(crate::http::Router) -> crate::http::Router + Send>);

    /// Store plugin-specific data for later retrieval.
    ///
    /// Plugins can store arbitrary data keyed by type. This data persists
    /// through the builder lifecycle and can be retrieved during controller
    /// registration or serve hooks.
    ///
    /// The data must be `Send + Sync` to be accessible across threads.
    fn store_plugin_data(&mut self, data: Box<dyn Any + Send + Sync>);

    /// Add a serve hook that runs when the server starts.
    ///
    /// The `start_fn` is a function pointer (stored as `usize`) with signature:
    /// `fn(Vec<ScheduledTaskDef<T>>, T, CancellationToken)` where `T` is the state type.
    ///
    /// The builder will:
    /// 1. Retrieve the `TaskRegistryHandle` from plugin data
    /// 2. Extract and downcast tasks to the correct type
    /// 3. Call `start_fn` with the tasks, state, and token
    ///
    /// # Safety
    ///
    /// The `start_fn` must be a valid function pointer with the signature above.
    fn add_serve_hook(&mut self, start_fn: usize, token: CancellationToken);

    /// Add a shutdown hook that runs when the server stops.
    fn add_shutdown_hook(&mut self, hook: Box<dyn FnOnce() + Send>);
}
