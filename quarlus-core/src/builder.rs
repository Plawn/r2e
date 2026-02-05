use crate::beans::{Bean, BeanRegistry, BeanState};
use crate::controller::Controller;
use crate::lifecycle::{ShutdownHook, StartupHook};
use crate::plugin::{DeferredInstallContext, DeferredPlugin, Plugin, PreStatePlugin};
use crate::type_list::{BuildableFrom, TCons, TNil};
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::info;

type ConsumerReg<T> =
    Box<dyn FnOnce(T) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send>;

type LayerFn = Box<dyn FnOnce(crate::http::Router) -> crate::http::Router + Send>;

/// A serve hook that receives tasks and starts them.
/// Tasks already have their state captured, so only the token is needed.
type ServeHook = Box<dyn FnOnce(Vec<Box<dyn Any + Send>>, CancellationToken) + Send>;

/// Marker type: application state has not been set yet.
///
/// `AppBuilder<NoState>` is the initial phase returned by [`AppBuilder::new()`].
/// Call [`.with_state()`](AppBuilder::with_state) or [`.build_state()`](AppBuilder::build_state)
/// to transition to `AppBuilder<T>`.
#[derive(Clone)]
pub struct NoState;

/// Shared configuration that is independent of the application state type.
struct BuilderConfig {
    config: Option<crate::config::QuarlusConfig>,
    custom_layers: Vec<LayerFn>,
    bean_registry: BeanRegistry,
    /// Deferred plugins to be installed after state resolution.
    deferred_plugins: Vec<DeferredPlugin>,
    /// Plugin data storage (type-erased, keyed by TypeId).
    plugin_data: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

/// Builder for assembling a Quarlus application.
///
/// Collects state, controller routes, and Tower layers, then produces an
/// `axum::Router` (or starts serving directly) with everything wired together.
///
/// # Two-phase builder
///
/// The builder starts in the `NoState` phase (`AppBuilder<NoState>`), where
/// you can call [`provide()`](Self::provide), [`with_bean()`](Self::with_bean),
/// and state-independent configuration methods. Transition to a typed phase
/// via:
///
/// - [`.with_state(state)`](AppBuilder::<NoState>::with_state) — provide a pre-built state directly.
/// - [`.build_state::<S>()`](AppBuilder::<NoState>::build_state) — resolve the bean graph and build state.
///
/// Once in the typed phase (`AppBuilder<T>`), you can register controllers,
/// install plugins via [`.with()`](Self::with), add hooks, and call `.build()`
/// or `.serve()`.
pub struct AppBuilder<T: Clone + Send + Sync + 'static = NoState, P = TNil> {
    shared: BuilderConfig,
    state: Option<T>,
    routes: Vec<crate::http::Router<T>>,
    pre_auth_guard_fns: Vec<Box<dyn FnOnce(crate::http::Router<T>, &T) -> crate::http::Router<T> + Send>>,
    startup_hooks: Vec<StartupHook<T>>,
    shutdown_hooks: Vec<ShutdownHook>,
    route_metadata: Vec<Vec<crate::openapi::RouteInfo>>,
    openapi_builder:
        Option<Box<dyn FnOnce(Vec<Vec<crate::openapi::RouteInfo>>) -> crate::http::Router<T> + Send>>,
    consumer_registrations: Vec<ConsumerReg<T>>,
    /// Serve hooks from plugins (called when server starts).
    /// Tasks already capture their state, so only the token is needed.
    serve_hooks: Vec<ServeHook>,
    /// Shutdown hooks from plugins.
    plugin_shutdown_hooks: Vec<Box<dyn FnOnce() + Send>>,
    _provided: PhantomData<P>,
}

// ── NoState phase (pre-state) ───────────────────────────────────────────────

impl AppBuilder<NoState, TNil> {
    /// Create a new, empty builder in the pre-state phase.
    pub fn new() -> Self {
        Self {
            shared: BuilderConfig {
                config: None,
                custom_layers: Vec::new(),
                bean_registry: BeanRegistry::new(),
                deferred_plugins: Vec::new(),
                plugin_data: HashMap::new(),
            },
            state: None,
            routes: Vec::new(),
            pre_auth_guard_fns: Vec::new(),
            startup_hooks: Vec::new(),
            shutdown_hooks: Vec::new(),
            route_metadata: Vec::new(),
            openapi_builder: None,
            consumer_registrations: Vec::new(),
            serve_hooks: Vec::new(),
            plugin_shutdown_hooks: Vec::new(),
            _provided: PhantomData,
        }
    }
}

impl<P> AppBuilder<NoState, P> {
    /// Internal: reconstruct the builder with an updated provider type list.
    fn with_updated_provider<NewP>(self) -> AppBuilder<NoState, NewP> {
        AppBuilder {
            shared: self.shared,
            state: None,
            routes: self.routes,
            pre_auth_guard_fns: self.pre_auth_guard_fns,
            startup_hooks: self.startup_hooks,
            shutdown_hooks: self.shutdown_hooks,
            route_metadata: self.route_metadata,
            openapi_builder: self.openapi_builder,
            consumer_registrations: self.consumer_registrations,
            serve_hooks: self.serve_hooks,
            plugin_shutdown_hooks: self.plugin_shutdown_hooks,
            _provided: PhantomData,
        }
    }

    /// Provide a pre-built bean instance.
    ///
    /// The instance will be available in the [`BeanContext`](crate::beans::BeanContext)
    /// for beans that depend on type `B`, and will be pulled into the state
    /// struct when [`build_state`](Self::build_state) is called.
    pub fn provide<B: Clone + Send + Sync + 'static>(mut self, bean: B) -> AppBuilder<NoState, TCons<B, P>> {
        self.shared.bean_registry.provide(bean);
        self.with_updated_provider()
    }

    /// Register a bean type for automatic construction.
    ///
    /// The bean's dependencies will be resolved from other beans and
    /// provided instances when [`build_state`](Self::build_state) is called.
    pub fn with_bean<B: Bean>(mut self) -> AppBuilder<NoState, TCons<B, P>> {
        self.shared.bean_registry.register::<B>();
        self.with_updated_provider()
    }

    /// Install a [`PreStatePlugin`] that provides beans and optionally defers setup.
    ///
    /// Pre-state plugins run before `build_state()` is called. They can:
    /// - Provide bean instances to the bean registry
    /// - Register deferred actions (like scheduler setup) that execute after state resolution
    ///
    /// # Example
    ///
    /// ```ignore
    /// use quarlus_scheduler::Scheduler;
    ///
    /// AppBuilder::new()
    ///     .with_plugin(Scheduler)  // Provides CancellationToken
    ///     .build_state::<Services, _>()
    /// ```
    pub fn with_plugin<Pl: PreStatePlugin>(self, plugin: Pl) -> AppBuilder<NoState, TCons<Pl::Provided, P>> {
        plugin.pre_install(self)
    }

    /// Add a deferred plugin to be installed after state resolution.
    ///
    /// This is called by [`PreStatePlugin`] implementations to register setup
    /// that needs to run after `build_state()` is called.
    ///
    /// # Example
    ///
    /// ```ignore
    /// impl PreStatePlugin for MyPlugin {
    ///     type Provided = MyToken;
    ///
    ///     fn pre_install<P>(self, app: AppBuilder<NoState, P>) -> AppBuilder<NoState, TCons<Self::Provided, P>> {
    ///         let token = MyToken::new();
    ///         let plugin = DeferredPlugin::new(
    ///             MySetupData { token: token.clone() },
    ///             MyInstaller,
    ///         );
    ///         app.provide(token).add_deferred_plugin(plugin)
    ///     }
    /// }
    /// ```
    pub fn add_deferred_plugin(mut self, plugin: DeferredPlugin) -> Self {
        self.shared.deferred_plugins.push(plugin);
        self
    }

    /// Resolve the bean dependency graph and build the application state.
    ///
    /// Consumes the bean registry, topologically sorts all beans, constructs
    /// them in order, and assembles the state struct via
    /// [`BeanState::from_context()`](crate::beans::BeanState::from_context).
    ///
    /// # Panics
    ///
    /// Panics if the bean graph has cycles, missing dependencies, or
    /// duplicate registrations. Use [`try_build_state`](Self::try_build_state)
    /// for a non-panicking alternative.
    pub fn build_state<S, Idx>(self) -> AppBuilder<S>
    where
        S: BeanState + BuildableFrom<P, Idx>,
    {
        self.try_build_state()
            .expect("Failed to resolve bean dependency graph")
    }

    /// Resolve the bean dependency graph and build the application state,
    /// returning an error instead of panicking on resolution failure.
    pub fn try_build_state<S, Idx>(
        mut self,
    ) -> Result<AppBuilder<S>, crate::beans::BeanError>
    where
        S: BeanState + BuildableFrom<P, Idx>,
    {
        let registry = std::mem::replace(&mut self.shared.bean_registry, BeanRegistry::new());
        let ctx = registry.resolve()?;
        let state = S::from_context(&ctx);
        Ok(AppBuilder::<S>::from_pre(self.shared, state))
    }

    /// Provide a pre-built state directly (backward-compatible path).
    ///
    /// This skips the bean graph entirely. The bean registry is discarded.
    /// No compile-time provision checking is performed.
    pub fn with_state<S: Clone + Send + Sync + 'static>(self, state: S) -> AppBuilder<S> {
        AppBuilder::<S>::from_pre(self.shared, state)
    }
}

impl Default for AppBuilder<NoState, TNil> {
    fn default() -> Self {
        Self::new()
    }
}

// ── Typed phase (state resolved) ────────────────────────────────────────────

impl<T: Clone + Send + Sync + 'static> AppBuilder<T> {
    /// Internal: construct a typed builder from the pre-state shared config.
    fn from_pre(mut shared: BuilderConfig, state: T) -> Self {
        // Take the deferred plugins before creating the builder.
        let deferred_plugins = std::mem::take(&mut shared.deferred_plugins);

        // Drop the bean registry since it's been consumed.
        shared.bean_registry = BeanRegistry::new();

        let mut builder = Self {
            shared,
            state: Some(state),
            routes: Vec::new(),
            pre_auth_guard_fns: Vec::new(),
            startup_hooks: Vec::new(),
            shutdown_hooks: Vec::new(),
            route_metadata: Vec::new(),
            openapi_builder: None,
            consumer_registrations: Vec::new(),
            serve_hooks: Vec::new(),
            plugin_shutdown_hooks: Vec::new(),
            _provided: PhantomData,
        };

        // Install deferred plugins.
        for plugin in deferred_plugins {
            let mut ctx = InstallContext {
                layers: &mut builder.shared.custom_layers,
                plugin_data: &mut builder.shared.plugin_data,
                serve_hooks: &mut builder.serve_hooks,
                shutdown_hooks: &mut builder.plugin_shutdown_hooks,
            };
            plugin.installer.install(plugin.data, &mut ctx);
        }

        builder
    }
}

/// Context for installing deferred plugins into a typed builder.
struct InstallContext<'a> {
    layers: &'a mut Vec<LayerFn>,
    plugin_data: &'a mut HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    serve_hooks: &'a mut Vec<ServeHook>,
    shutdown_hooks: &'a mut Vec<Box<dyn FnOnce() + Send>>,
}

impl DeferredInstallContext for InstallContext<'_> {
    fn add_layer(&mut self, layer: Box<dyn FnOnce(crate::http::Router) -> crate::http::Router + Send>) {
        self.layers.push(layer);
    }

    fn store_plugin_data(&mut self, data: Box<dyn Any + Send + Sync>) {
        // Use the concrete type's TypeId as the key.
        let type_id = (*data).type_id();
        self.plugin_data.insert(type_id, data);
    }

    fn add_serve_hook(&mut self, start_fn: usize, token: CancellationToken) {
        // Reinterpret the function pointer.
        // This is safe because tasks now capture their state, so the function
        // only needs tasks and token (no generic T parameter needed).
        type StartFn = fn(Vec<Box<dyn Any + Send>>, CancellationToken);
        let start_fn: StartFn = unsafe { std::mem::transmute(start_fn) };

        // Create a closure that will be called at serve time.
        let closure: ServeHook = Box::new(move |tasks, token_at_serve| {
            // Note: we use the token from add_serve_hook, not the one passed at serve time
            // (they should be the same token anyway)
            let _ = token_at_serve;
            start_fn(tasks, token);
        });

        self.serve_hooks.push(closure);
    }

    fn add_shutdown_hook(&mut self, hook: Box<dyn FnOnce() + Send>) {
        self.shutdown_hooks.push(hook);
    }
}

impl<T: Clone + Send + Sync + 'static> AppBuilder<T> {
    // ── Plugin system ───────────────────────────────────────────────────

    /// Install a [`Plugin`] into this builder.
    ///
    /// Plugins are composable units of functionality (CORS, tracing, health
    /// checks, etc.) that modify the builder. This replaces the old
    /// `with_cors()`, `with_tracing()`, etc. methods with a single, uniform
    /// entry point.
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
    pub fn with<Pl: Plugin<T>>(self, plugin: Pl) -> Self {
        plugin.install(self)
    }

    // ── Configuration ───────────────────────────────────────────────────

    /// Store a `QuarlusConfig` in the builder.
    ///
    /// The config is stored as an Axum extension and can be extracted via
    /// `FromRef` if the user state implements it. This is a convenience
    /// method — you can also embed `QuarlusConfig` directly in your state.
    pub fn with_config(mut self, config: crate::config::QuarlusConfig) -> Self {
        self.shared.config = Some(config);
        self
    }

    // ── Layer primitives ────────────────────────────────────────────────

    /// Apply a Tower layer to the entire application.
    ///
    /// The layer is applied during `build()`. Multiple calls are applied in
    /// order. The layer must satisfy the same bounds as [`axum::Router::layer`].
    ///
    /// # Example
    ///
    /// ```ignore
    /// use tower_http::timeout::TimeoutLayer;
    /// use std::time::Duration;
    ///
    /// AppBuilder::new()
    ///     .with_layer(TimeoutLayer::new(Duration::from_secs(30)))
    /// ```
    pub fn with_layer<L>(mut self, layer: L) -> Self
    where
        L: tower::Layer<crate::http::routing::Route> + Clone + Send + Sync + 'static,
        L::Service: Clone
            + tower::Service<crate::http::header::HttpRequest<crate::http::body::Body>>
            + Send
            + Sync
            + 'static,
        <L::Service as tower::Service<crate::http::header::HttpRequest<crate::http::body::Body>>>::Response:
            crate::http::response::IntoResponse + 'static,
        <L::Service as tower::Service<crate::http::header::HttpRequest<crate::http::body::Body>>>::Error:
            Into<std::convert::Infallible> + 'static,
        <L::Service as tower::Service<crate::http::header::HttpRequest<crate::http::body::Body>>>::Future:
            Send + 'static,
    {
        self.shared
            .custom_layers
            .push(Box::new(move |router| router.layer(layer)));
        self
    }

    /// Apply a custom transformation to the router.
    ///
    /// This is an escape hatch for cases where `with_layer` is too
    /// restrictive. The closure receives the `axum::Router` and must return
    /// a new `axum::Router`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// AppBuilder::new()
    ///     .with_layer_fn(|router| {
    ///         router.layer(some_complex_layer)
    ///     })
    /// ```
    pub fn with_layer_fn<F>(mut self, f: F) -> Self
    where
        F: FnOnce(crate::http::Router) -> crate::http::Router + Send + 'static,
    {
        self.shared.custom_layers.push(Box::new(f));
        self
    }

    /// Semantic alias for [`with_layer_fn`](Self::with_layer_fn) when using
    /// `tower::ServiceBuilder`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use tower::ServiceBuilder;
    /// use tower_http::timeout::TimeoutLayer;
    ///
    /// AppBuilder::new()
    ///     .with_service_builder(|router| {
    ///         router.layer(
    ///             ServiceBuilder::new()
    ///                 .layer(TimeoutLayer::new(Duration::from_secs(30)))
    ///         )
    ///     })
    /// ```
    pub fn with_service_builder<F>(self, f: F) -> Self
    where
        F: FnOnce(crate::http::Router) -> crate::http::Router + Send + 'static,
    {
        self.with_layer_fn(f)
    }

    // ── State-dependent methods ─────────────────────────────────────────

    /// Register a startup hook that runs before the server starts listening.
    ///
    /// # Example
    ///
    /// ```ignore
    /// AppBuilder::new()
    ///     .on_start(|state| Box::pin(async move {
    ///         sqlx::query("SELECT 1").execute(&state.pool).await?;
    ///         Ok(())
    ///     }))
    /// ```
    pub fn on_start<F, Fut>(mut self, hook: F) -> Self
    where
        F: FnOnce(T) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>>
            + Send
            + 'static,
    {
        self.startup_hooks
            .push(Box::new(move |state| Box::pin(hook(state))));
        self
    }

    /// Register a shutdown hook that runs after the server stops.
    ///
    /// # Example
    ///
    /// ```ignore
    /// AppBuilder::new()
    ///     .on_stop(|| Box::pin(async { tracing::info!("Bye"); }))
    /// ```
    pub fn on_stop<F, Fut>(mut self, hook: F) -> Self
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        self.shutdown_hooks
            .push(Box::new(move || Box::pin(hook())));
        self
    }

    /// Register a raw `axum::Router` fragment to be merged into the application.
    pub fn register_routes(mut self, router: crate::http::Router<T>) -> Self {
        self.routes.push(router);
        self
    }

    /// Get plugin data by type.
    ///
    /// Returns a reference to plugin data previously stored via
    /// `DeferredInstallContext::store_plugin_data`.
    pub fn get_plugin_data<D: Any + Send + Sync + 'static>(&self) -> Option<&D> {
        self.shared
            .plugin_data
            .get(&TypeId::of::<D>())
            .and_then(|boxed| boxed.downcast_ref::<D>())
    }

    /// Register a [`Controller`] whose routes will be merged into the application.
    ///
    /// This also collects event consumers and scheduled task definitions
    /// declared on the controller, so that they are started automatically
    /// by `serve()`.
    pub fn register_controller<C: Controller<T>>(mut self) -> Self {
        self.routes.push(C::routes());
        self.pre_auth_guard_fns
            .push(Box::new(|router, state| C::apply_pre_auth_guards(router, state)));
        self.route_metadata.push(C::route_metadata());
        self.consumer_registrations
            .push(Box::new(|state| C::register_consumers(state)));

        // Collect scheduled tasks (type-erased) and add to the task registry if present.
        // Tasks capture the state, so we need to pass it here.
        if let Some(state) = &self.state {
            let boxed_tasks = C::scheduled_tasks_boxed(state);
            if !boxed_tasks.is_empty() {
                if let Some(registry) = self.get_plugin_data::<TaskRegistryHandle>() {
                    registry.add_boxed(boxed_tasks);
                } else {
                    tracing::warn!(
                        controller = std::any::type_name::<C>(),
                        "Scheduled tasks found but no scheduler installed. \
                         Add `.with_plugin(Scheduler)` before build_state()."
                    );
                }
            }
        }

        self
    }

    /// Register a deferred OpenAPI route builder.
    ///
    /// The callback receives all route metadata collected from `register_controller`
    /// calls (in order) and must return a `Router<T>` to merge into the app.
    /// Called at `build()` time, so registration order does not matter.
    pub fn with_openapi_builder<F>(mut self, f: F) -> Self
    where
        F: FnOnce(Vec<Vec<crate::openapi::RouteInfo>>) -> crate::http::Router<T> + Send + 'static,
    {
        self.openapi_builder = Some(Box::new(f));
        self
    }

    /// Assemble the final `axum::Router` from all registered routes and layers.
    ///
    /// # Panics
    ///
    /// Panics if [`with_state`](AppBuilder::<NoState>::with_state) or
    /// [`build_state`](AppBuilder::<NoState>::build_state) was never called.
    pub fn build(self) -> crate::http::Router {
        self.build_inner().0
    }

    fn build_inner(
        self,
    ) -> (
        crate::http::Router,
        Vec<StartupHook<T>>,
        Vec<ShutdownHook>,
        Vec<ConsumerReg<T>>,
        Vec<ServeHook>,
        Vec<Box<dyn FnOnce() + Send>>,
        HashMap<TypeId, Box<dyn Any + Send + Sync>>,
        T,
    ) {
        let state = self
            .state
            .expect("AppBuilder: state must be set before build");

        let mut router = crate::http::Router::new();

        // Merge all controller / manual routes.
        for r in self.routes {
            router = router.merge(r);
        }

        // Apply pre-auth guard layers (before state finalization).
        for guard_fn in self.pre_auth_guard_fns {
            router = guard_fn(router, &state);
        }

        // Invoke the deferred OpenAPI builder, if registered.
        if let Some(builder) = self.openapi_builder {
            let openapi_router = builder(self.route_metadata);
            router = router.merge(openapi_router);
        }

        // Apply the application state.
        let mut app = router.with_state(state.clone());

        // Apply layers (in registration order).
        for layer_fn in self.shared.custom_layers {
            app = layer_fn(app);
        }

        (
            app,
            self.startup_hooks,
            self.shutdown_hooks,
            self.consumer_registrations,
            self.serve_hooks,
            self.plugin_shutdown_hooks,
            self.shared.plugin_data,
            state,
        )
    }

    /// Build the application and start serving on the given address.
    ///
    /// Runs startup hooks before listening, and shutdown hooks after
    /// graceful shutdown completes.
    pub async fn serve(self, addr: &str) -> Result<(), Box<dyn std::error::Error>> {
        let (
            app,
            startup_hooks,
            shutdown_hooks,
            consumer_regs,
            serve_hooks,
            plugin_shutdown_hooks,
            plugin_data,
            state,
        ) = self.build_inner();

        // Register event consumers
        for reg in consumer_regs {
            reg(state.clone()).await;
        }

        // Call serve hooks (e.g., scheduler starts tasks).
        // Tasks already have their state captured, so we just need to pass the token.
        for hook in serve_hooks {
            if let Some(registry) = plugin_data
                .get(&TypeId::of::<TaskRegistryHandle>())
                .and_then(|d| d.downcast_ref::<TaskRegistryHandle>())
            {
                let boxed_tasks = registry.take_all();
                if !boxed_tasks.is_empty() {
                    // The token is captured in the hook closure
                    hook(boxed_tasks, CancellationToken::new());
                }
            }
        }

        // Run startup hooks
        for hook in startup_hooks {
            hook(state.clone())
                .await
                .map_err(|e| -> Box<dyn std::error::Error> { e })?;
        }

        let listener = tokio::net::TcpListener::bind(addr).await?;
        info!(%addr, "Quarlus server listening");
        crate::http::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await?;

        // Run plugin shutdown hooks (e.g., cancel scheduler)
        for hook in plugin_shutdown_hooks {
            hook();
        }

        // Run user shutdown hooks
        for hook in shutdown_hooks {
            hook().await;
        }

        info!("Quarlus server stopped");
        Ok(())
    }
}

/// Handle to a task registry for collecting scheduled tasks.
///
/// This is stored in plugin_data by the scheduler plugin and used by
/// `register_controller` to collect scheduled tasks. It's cloneable
/// (internally Arc) so it can be shared.
#[derive(Clone)]
pub struct TaskRegistryHandle {
    inner: Arc<Mutex<Vec<Box<dyn Any + Send>>>>,
}

impl TaskRegistryHandle {
    /// Create a new empty task registry handle.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Add type-erased tasks to the registry.
    pub fn add_boxed(&self, tasks: Vec<Box<dyn Any + Send>>) {
        self.inner.lock().unwrap().extend(tasks);
    }

    /// Take all tasks from the registry, leaving it empty.
    pub fn take_all(&self) -> Vec<Box<dyn Any + Send>> {
        std::mem::take(&mut *self.inner.lock().unwrap())
    }
}

impl Default for TaskRegistryHandle {
    fn default() -> Self {
        Self::new()
    }
}

/// Wait for a shutdown signal (Ctrl-C or SIGTERM on Unix).
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to listen for Ctrl-C");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to listen for SIGTERM")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("Shutdown signal received, starting graceful shutdown");
}
