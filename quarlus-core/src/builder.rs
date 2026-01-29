use crate::beans::{Bean, BeanRegistry, BeanState};
use crate::controller::Controller;
use crate::lifecycle::{ShutdownHook, StartupHook};
use crate::plugin::Plugin;
use crate::scheduling::{ScheduledTaskDef, SchedulerStartFn, SchedulerStopFn};
use crate::type_list::{BuildableFrom, TCons, TNil};
use std::marker::PhantomData;
use tracing::info;

type ConsumerReg<T> =
    Box<dyn FnOnce(T) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send>;

type LayerFn = Box<dyn FnOnce(crate::http::Router) -> crate::http::Router + Send>;

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
    startup_hooks: Vec<StartupHook<T>>,
    shutdown_hooks: Vec<ShutdownHook>,
    route_metadata: Vec<Vec<crate::openapi::RouteInfo>>,
    openapi_builder:
        Option<Box<dyn FnOnce(Vec<Vec<crate::openapi::RouteInfo>>) -> crate::http::Router<T> + Send>>,
    consumer_registrations: Vec<ConsumerReg<T>>,
    scheduled_task_defs: Vec<ScheduledTaskDef<T>>,
    scheduler_starter: Option<SchedulerStartFn<T>>,
    scheduler_stopper: Option<SchedulerStopFn>,
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
            },
            state: None,
            routes: Vec::new(),
            startup_hooks: Vec::new(),
            shutdown_hooks: Vec::new(),
            route_metadata: Vec::new(),
            openapi_builder: None,
            consumer_registrations: Vec::new(),
            scheduled_task_defs: Vec::new(),
            scheduler_starter: None,
            scheduler_stopper: None,
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
            startup_hooks: self.startup_hooks,
            shutdown_hooks: self.shutdown_hooks,
            route_metadata: self.route_metadata,
            openapi_builder: self.openapi_builder,
            consumer_registrations: self.consumer_registrations,
            scheduled_task_defs: self.scheduled_task_defs,
            scheduler_starter: self.scheduler_starter,
            scheduler_stopper: self.scheduler_stopper,
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
        // Drop the bean registry since it's been consumed.
        shared.bean_registry = BeanRegistry::new();
        Self {
            shared,
            state: Some(state),
            routes: Vec::new(),
            startup_hooks: Vec::new(),
            shutdown_hooks: Vec::new(),
            route_metadata: Vec::new(),
            openapi_builder: None,
            consumer_registrations: Vec::new(),
            scheduled_task_defs: Vec::new(),
            scheduler_starter: None,
            scheduler_stopper: None,
            _provided: PhantomData,
        }
    }

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

    /// Register a [`Controller`] whose routes will be merged into the application.
    ///
    /// This also collects event consumers and scheduled task definitions
    /// declared on the controller, so that they are started automatically
    /// by `serve()`.
    pub fn register_controller<C: Controller<T>>(mut self) -> Self {
        self.routes.push(C::routes());
        self.route_metadata.push(C::route_metadata());
        self.consumer_registrations
            .push(Box::new(|state| C::register_consumers(state)));
        self.scheduled_task_defs.extend(C::scheduled_tasks());
        self
    }

    /// Set the scheduler backend (starter + stopper).
    ///
    /// Called by the scheduler plugin to provide the runtime that actually
    /// spawns Tokio tasks for each [`ScheduledTaskDef`].
    pub fn set_scheduler_backend(
        mut self,
        starter: SchedulerStartFn<T>,
        stopper: SchedulerStopFn,
    ) -> Self {
        self.scheduler_starter = Some(starter);
        self.scheduler_stopper = Some(stopper);
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
    pub fn build(mut self) -> crate::http::Router {
        if !self.scheduled_task_defs.is_empty() && self.scheduler_starter.is_none() {
            tracing::warn!(
                "Scheduled tasks were registered but no scheduler backend was installed. \
                 Add `.with(Scheduler)` (from quarlus-scheduler) to start them."
            );
        }
        // Discard scheduler fields — build() returns a bare Router.
        self.scheduled_task_defs.clear();
        self.scheduler_starter = None;
        self.scheduler_stopper = None;
        self.build_inner().0
    }

    fn build_inner(
        self,
    ) -> (
        crate::http::Router,
        Vec<StartupHook<T>>,
        Vec<ShutdownHook>,
        Vec<ConsumerReg<T>>,
        Vec<ScheduledTaskDef<T>>,
        Option<SchedulerStartFn<T>>,
        Option<SchedulerStopFn>,
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
            self.scheduled_task_defs,
            self.scheduler_starter,
            self.scheduler_stopper,
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
            scheduled_task_defs,
            scheduler_starter,
            scheduler_stopper,
            state,
        ) = self.build_inner();

        // Register event consumers
        for reg in consumer_regs {
            reg(state.clone()).await;
        }

        // Start scheduled tasks
        if !scheduled_task_defs.is_empty() {
            if let Some(starter) = scheduler_starter {
                info!(
                    count = scheduled_task_defs.len(),
                    "Starting scheduled tasks"
                );
                starter(scheduled_task_defs, state.clone());
            } else {
                tracing::warn!(
                    "Scheduled tasks were registered but no scheduler backend was installed. \
                     Add `.with(Scheduler)` (from quarlus-scheduler) to start them."
                );
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

        // Stop scheduler
        if let Some(stopper) = scheduler_stopper {
            stopper();
        }

        // Run shutdown hooks
        for hook in shutdown_hooks {
            hook().await;
        }

        info!("Quarlus server stopped");
        Ok(())
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
