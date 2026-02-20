use crate::beans::{AsyncBean, Bean, BeanRegistry, BeanState, Producer};
use crate::controller::Controller;
use crate::lifecycle::{ShutdownHook, StartupHook};
use crate::meta::MetaRegistry;
use crate::service::ServiceComponent;
#[allow(deprecated)]
use crate::plugin::{DeferredAction, DeferredContext, DeferredInstallContext, DeferredPlugin, Plugin, RawPreStatePlugin};
use crate::type_list::{AllSatisfied, BuildableFrom, TAppend, TCons, TNil};
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::info;

type ConsumerReg<T> =
    Box<dyn FnOnce(T) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send>;

type LayerFn = Box<dyn FnOnce(crate::http::Router) -> crate::http::Router + Send>;

/// A meta consumer that drains typed metadata from the registry and returns
/// a router fragment to be merged into the application.
type MetaConsumer<T> = Box<dyn FnOnce(&MetaRegistry) -> crate::http::Router<T> + Send>;

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
    config: Option<crate::config::R2eConfig>,
    custom_layers: Vec<LayerFn>,
    bean_registry: BeanRegistry,
    /// Deferred actions to be executed after state resolution.
    deferred_actions: Vec<DeferredAction>,
    /// Legacy deferred plugins (deprecated, for backward compatibility).
    #[allow(deprecated)]
    deferred_plugins: Vec<DeferredPlugin>,
    /// Plugin data storage (type-erased, keyed by TypeId).
    plugin_data: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    /// Name of the last plugin that should be installed last (for ordering validation).
    last_plugin_name: Option<&'static str>,
    /// Whether to install a trailing-slash normalization fallback.
    normalize_path: bool,
}

/// Builder for assembling a R2E application.
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
pub struct AppBuilder<T: Clone + Send + Sync + 'static = NoState, P = TNil, R = TNil> {
    shared: BuilderConfig,
    state: Option<T>,
    routes: Vec<crate::http::Router<T>>,
    pre_auth_guard_fns: Vec<Box<dyn FnOnce(crate::http::Router<T>, &T) -> crate::http::Router<T> + Send>>,
    startup_hooks: Vec<StartupHook<T>>,
    shutdown_hooks: Vec<ShutdownHook>,
    meta_registry: MetaRegistry,
    meta_consumers: Vec<MetaConsumer<T>>,
    consumer_registrations: Vec<ConsumerReg<T>>,
    /// Serve hooks from plugins (called when server starts).
    /// Tasks already capture their state, so only the token is needed.
    serve_hooks: Vec<ServeHook>,
    /// Shutdown hooks from plugins.
    plugin_shutdown_hooks: Vec<Box<dyn FnOnce() + Send>>,
    _provided: PhantomData<P>,
    _required: PhantomData<R>,
}

// ── NoState phase (pre-state) ───────────────────────────────────────────────

impl AppBuilder<NoState, TNil, TNil> {
    /// Create a new, empty builder in the pre-state phase.
    pub fn new() -> Self {
        Self {
            shared: BuilderConfig {
                config: None,
                custom_layers: Vec::new(),
                bean_registry: BeanRegistry::new(),
                deferred_actions: Vec::new(),
                deferred_plugins: Vec::new(),
                plugin_data: HashMap::new(),
                last_plugin_name: None,
                normalize_path: false,
            },
            state: None,
            routes: Vec::new(),
            pre_auth_guard_fns: Vec::new(),
            startup_hooks: Vec::new(),
            shutdown_hooks: Vec::new(),
            meta_registry: MetaRegistry::new(),
            meta_consumers: Vec::new(),
            consumer_registrations: Vec::new(),
            serve_hooks: Vec::new(),
            plugin_shutdown_hooks: Vec::new(),
            _provided: PhantomData,
            _required: PhantomData,
        }
    }
}

impl<P, R> AppBuilder<NoState, P, R> {
    /// Reconstruct the builder with updated type-level tracking parameters.
    ///
    /// Since `P` and `R` are phantom types used only for compile-time bean graph
    /// validation, this is a zero-cost conversion that just changes the markers.
    #[doc(hidden)]
    pub fn with_updated_types<NewP, NewR>(self) -> AppBuilder<NoState, NewP, NewR> {
        AppBuilder {
            shared: self.shared,
            state: None,
            routes: self.routes,
            pre_auth_guard_fns: self.pre_auth_guard_fns,
            startup_hooks: self.startup_hooks,
            shutdown_hooks: self.shutdown_hooks,
            meta_registry: self.meta_registry,
            meta_consumers: self.meta_consumers,
            consumer_registrations: self.consumer_registrations,
            serve_hooks: self.serve_hooks,
            plugin_shutdown_hooks: self.plugin_shutdown_hooks,
            _provided: PhantomData,
            _required: PhantomData,
        }
    }

    /// Provide a pre-built bean instance.
    ///
    /// The instance will be available in the [`BeanContext`](crate::beans::BeanContext)
    /// for beans that depend on type `B`, and will be pulled into the state
    /// struct when [`build_state`](Self::build_state) is called.
    pub fn provide<B: Clone + Send + Sync + 'static>(mut self, bean: B) -> AppBuilder<NoState, TCons<B, P>, R> {
        self.shared.bean_registry.provide(bean);
        self.with_updated_types()
    }

    /// Register a (sync) bean type for automatic construction.
    ///
    /// The bean's dependencies will be resolved from other beans and
    /// provided instances when [`build_state`](Self::build_state) is called.
    pub fn with_bean<B: Bean>(mut self) -> AppBuilder<NoState, TCons<B, P>, <R as TAppend<B::Deps>>::Output>
    where
        R: TAppend<B::Deps>,
    {
        self.shared.bean_registry.register::<B>();
        self.with_updated_types()
    }

    /// Register an async bean type for automatic construction.
    ///
    /// The bean's async constructor will be awaited during
    /// [`build_state`](Self::build_state).
    pub fn with_async_bean<B: AsyncBean>(mut self) -> AppBuilder<NoState, TCons<B, P>, <R as TAppend<B::Deps>>::Output>
    where
        R: TAppend<B::Deps>,
    {
        self.shared.bean_registry.register_async::<B>();
        self.with_updated_types()
    }

    /// Register a producer for automatic construction of its output type.
    ///
    /// The producer creates an instance of `Pr::Output` during
    /// [`build_state`](Self::build_state). The output type (not the producer
    /// struct) is tracked in the provision list.
    pub fn with_producer<Pr: Producer>(mut self) -> AppBuilder<NoState, TCons<Pr::Output, P>, <R as TAppend<Pr::Deps>>::Output>
    where
        R: TAppend<Pr::Deps>,
    {
        self.shared.bean_registry.register_producer::<Pr>();
        self.with_updated_types()
    }

    /// Register a bean via factory closure with access to [`R2eConfig`](crate::config::R2eConfig).
    ///
    /// The closure receives a reference to the resolved config and returns
    /// a bean instance. Use this when you need programmatic construction
    /// based on configuration values.
    ///
    /// # Example
    ///
    /// ```ignore
    /// AppBuilder::new()
    ///     .provide(config)
    ///     .with_bean_factory(|config: &R2eConfig| {
    ///         let url = config.get::<String>("redis.url").unwrap();
    ///         RedisClient::new(&url)
    ///     })
    ///     .build_state::<Services, _, _>().await
    /// ```
    pub fn with_bean_factory<B, F>(mut self, factory: F) -> AppBuilder<NoState, TCons<B, P>, <R as TAppend<TCons<crate::config::R2eConfig, TNil>>>::Output>
    where
        B: Clone + Send + Sync + 'static,
        F: FnOnce(&crate::config::R2eConfig) -> B + Send + 'static,
        R: TAppend<TCons<crate::config::R2eConfig, TNil>>,
    {
        self.shared
            .bean_registry
            .provide_factory_with_config::<B, F>(factory);
        self.with_updated_types()
    }

    /// Enable bean override mode (useful for testing).
    ///
    /// When enabled, duplicate bean registrations are allowed and the last
    /// registration wins. Must be called before [`override_provide`](Self::override_provide).
    pub fn allow_bean_override(mut self) -> Self {
        self.shared.bean_registry.allow_overrides = true;
        self
    }

    /// Override a previously registered bean with a pre-built instance.
    ///
    /// Requires [`allow_bean_override`](Self::allow_bean_override) to be called first.
    ///
    /// # Panics
    ///
    /// Panics if `allow_bean_override()` was not called.
    pub fn override_provide<B: Clone + Send + Sync + 'static>(
        mut self,
        value: B,
    ) -> Self {
        assert!(
            self.shared.bean_registry.allow_overrides,
            "Call allow_bean_override() before override_provide()"
        );
        self.shared.bean_registry.provide(value);
        self
    }

    /// Install a pre-state plugin that provides beans and optionally defers setup.
    ///
    /// Accepts any [`PreStatePlugin`](crate::PreStatePlugin) (simple, single-provision)
    /// or [`RawPreStatePlugin`] (advanced, multi-provision). Pre-state plugins run
    /// before `build_state()` is called. They can:
    /// - Provide bean instances to the bean registry
    /// - Register deferred actions (like scheduler setup) that execute after state resolution
    ///
    /// # Example
    ///
    /// ```ignore
    /// use r2e_scheduler::Scheduler;
    ///
    /// AppBuilder::new()
    ///     .plugin(Scheduler)  // Provides CancellationToken + ScheduledJobRegistry
    ///     .build_state::<Services, _, _>()
    ///     .await
    /// ```
    pub fn plugin<Pl: RawPreStatePlugin>(
        self,
        plugin: Pl,
    ) -> AppBuilder<NoState, <P as TAppend<Pl::Provisions>>::Output, <R as TAppend<Pl::Required>>::Output>
    where
        P: TAppend<Pl::Provisions>,
        R: TAppend<Pl::Required>,
    {
        plugin.install(self)
    }

    /// Alias for [`.plugin()`](Self::plugin) for pre-state plugins.
    ///
    /// # Deprecated
    ///
    /// Use [`.plugin()`](Self::plugin) instead.
    #[deprecated(since = "0.2.0", note = "Use .plugin() instead")]
    pub fn with_plugin<Pl: RawPreStatePlugin>(
        self,
        plugin: Pl,
    ) -> AppBuilder<NoState, <P as TAppend<Pl::Provisions>>::Output, <R as TAppend<Pl::Required>>::Output>
    where
        P: TAppend<Pl::Provisions>,
        R: TAppend<Pl::Required>,
    {
        plugin.install(self)
    }

    /// Add a deferred action to be executed after state resolution.
    ///
    /// This is called by [`PreStatePlugin`] implementations to register setup
    /// that needs to run after `build_state()` is called.
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
    pub fn add_deferred(mut self, action: DeferredAction) -> Self {
        self.shared.deferred_actions.push(action);
        self
    }

    /// Add a deferred plugin to be installed after state resolution.
    ///
    /// # Deprecated
    ///
    /// Use [`add_deferred`](Self::add_deferred) with [`DeferredAction`] instead.
    #[deprecated(since = "0.2.0", note = "Use add_deferred with DeferredAction instead")]
    #[allow(deprecated)]
    pub fn add_deferred_plugin(mut self, plugin: DeferredPlugin) -> Self {
        self.shared.deferred_plugins.push(plugin);
        self
    }

    /// Resolve the bean dependency graph and build the application state.
    ///
    /// Consumes the bean registry, topologically sorts all beans, constructs
    /// them in order (awaiting async beans/producers), and assembles the
    /// state struct via [`BeanState::from_context()`](crate::beans::BeanState::from_context).
    ///
    /// The `R` (requirements) type parameter is checked against `P` (provisions)
    /// at compile time via the [`AllSatisfied`] bound: every bean dependency
    /// must be present in the provision list. If a dependency is missing, the
    /// compiler emits an error.
    ///
    /// # Panics
    ///
    /// Panics if the bean graph has cycles, missing dependencies, or
    /// duplicate registrations. Use [`try_build_state`](Self::try_build_state)
    /// for a non-panicking alternative.
    pub async fn build_state<S, Idx, RIdx>(self) -> AppBuilder<S>
    where
        S: BeanState + BuildableFrom<P, Idx>,
        R: AllSatisfied<P, RIdx>,
    {
        self.try_build_state()
            .await
            .expect("Failed to resolve bean dependency graph")
    }

    /// Resolve the bean dependency graph and build the application state,
    /// returning an error instead of panicking on resolution failure.
    pub async fn try_build_state<S, Idx, RIdx>(
        mut self,
    ) -> Result<AppBuilder<S>, crate::beans::BeanError>
    where
        S: BeanState + BuildableFrom<P, Idx>,
        R: AllSatisfied<P, RIdx>,
    {
        let registry = std::mem::replace(&mut self.shared.bean_registry, BeanRegistry::new());
        let ctx = registry.resolve().await?;
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

impl Default for AppBuilder<NoState, TNil, TNil> {
    fn default() -> Self {
        Self::new()
    }
}

// ── Typed phase (state resolved) ────────────────────────────────────────────

impl<T: Clone + Send + Sync + 'static> AppBuilder<T> {
    /// Internal: construct a typed builder from the pre-state shared config.
    #[allow(deprecated)]
    fn from_pre(mut shared: BuilderConfig, state: T) -> Self {
        // Take the deferred actions and plugins before creating the builder.
        let deferred_actions = std::mem::take(&mut shared.deferred_actions);
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
            meta_registry: MetaRegistry::new(),
            meta_consumers: Vec::new(),
            consumer_registrations: Vec::new(),
            serve_hooks: Vec::new(),
            plugin_shutdown_hooks: Vec::new(),
            _provided: PhantomData,
            _required: PhantomData,
        };

        // Execute deferred actions (new API).
        for action in deferred_actions {
            let mut ctx = DeferredContext {
                layers: &mut builder.shared.custom_layers,
                plugin_data: &mut builder.shared.plugin_data,
                serve_hooks: &mut builder.serve_hooks,
                shutdown_hooks: &mut builder.plugin_shutdown_hooks,
            };
            (action.action)(&mut ctx);
        }

        // Install legacy deferred plugins (deprecated API).
        for plugin in deferred_plugins {
            let mut ctx = LegacyInstallContext {
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

/// Context for installing legacy deferred plugins into a typed builder.
#[allow(deprecated)]
struct LegacyInstallContext<'a> {
    layers: &'a mut Vec<LayerFn>,
    plugin_data: &'a mut HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    serve_hooks: &'a mut Vec<ServeHook>,
    shutdown_hooks: &'a mut Vec<Box<dyn FnOnce() + Send>>,
}

#[allow(deprecated)]
impl DeferredInstallContext for LegacyInstallContext<'_> {
    fn add_layer(&mut self, layer: Box<dyn FnOnce(crate::http::Router) -> crate::http::Router + Send>) {
        self.layers.push(layer);
    }

    fn store_plugin_data(&mut self, data: Box<dyn Any + Send + Sync>) {
        // Use the concrete type's TypeId as the key.
        let type_id = (*data).type_id();
        self.plugin_data.insert(type_id, data);
    }

    fn add_serve_hook(
        &mut self,
        hook: Box<dyn FnOnce(Vec<Box<dyn Any + Send>>, CancellationToken) + Send>,
    ) {
        self.serve_hooks.push(hook);
    }

    fn add_shutdown_hook(&mut self, hook: Box<dyn FnOnce() + Send>) {
        self.shutdown_hooks.push(hook);
    }
}

impl<T: Clone + Send + Sync + 'static> AppBuilder<T> {
    // ── Path normalization ──────────────────────────────────────────────

    /// Enable trailing-slash normalization via a router fallback.
    ///
    /// When enabled, requests to paths with a trailing slash (e.g. `/users/`)
    /// that don't match any route are re-dispatched with the slash stripped
    /// (e.g. `/users`). This can be installed at any point in the plugin chain.
    pub(crate) fn enable_normalize_path(mut self) -> Self {
        self.shared.normalize_path = true;
        self
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
    /// use r2e_core::plugins::{Cors, Tracing, Health, ErrorHandling, DevReload};
    ///
    /// AppBuilder::new()
    ///     .build_state::<Services>()
    ///     .await
    ///     .with(Health)
    ///     .with(Cors::permissive())
    ///     .with(Tracing)
    ///     .with(ErrorHandling)
    ///     .with(DevReload)
    /// ```
    pub fn with<Pl: Plugin>(mut self, plugin: Pl) -> Self {
        // Check if a "should be last" plugin was already installed
        if let Some(last_name) = self.shared.last_plugin_name {
            tracing::warn!(
                previous = last_name,
                current = Pl::name(),
                "Plugin {} should be installed last, but {} is being installed after it. \
                 This may cause unexpected behavior.",
                last_name,
                Pl::name(),
            );
        }

        // Track if this plugin should be last
        if Pl::should_be_last() {
            self.shared.last_plugin_name = Some(Pl::name());
        }

        plugin.install(self)
    }

    // ── Configuration ───────────────────────────────────────────────────

    /// Store a `R2eConfig` in the builder.
    ///
    /// The config is stored as an Axum extension and can be extracted via
    /// `FromRef` if the user state implements it. This is a convenience
    /// method — you can also embed `R2eConfig` directly in your state.
    pub fn with_config(mut self, config: crate::config::R2eConfig) -> Self {
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

    /// Escape hatch: merge a raw Axum router alongside controllers.
    ///
    /// Raw routes benefit from global plugins (Tracing, CORS, ErrorHandling)
    /// but do NOT get controller-level DI, interceptors, or guards.
    ///
    /// This is a convenience alias for [`register_routes`](Self::register_routes).
    pub fn merge_router(self, router: crate::http::Router<T>) -> Self {
        self.register_routes(router)
    }

    /// Spawn a background [`ServiceComponent`] that participates in DI.
    ///
    /// The service is constructed from the application state via
    /// [`ServiceComponent::from_state`] and started in a Tokio task during
    /// `on_start`. A [`CancellationToken`] is provided and cancelled
    /// automatically during shutdown.
    ///
    /// # Example
    ///
    /// ```ignore
    /// AppBuilder::new()
    ///     .build_state::<Services, _, _>().await
    ///     .spawn_service::<MetricsExporter>()
    ///     .serve("0.0.0.0:3000").await
    /// ```
    pub fn spawn_service<C: ServiceComponent<T>>(mut self) -> Self {
        let token = CancellationToken::new();
        let shutdown_token = token.clone();

        self = self.on_start(move |state| async move {
            let service = C::from_state(&state);
            tokio::spawn(service.start(token));
            Ok(())
        });

        self.plugin_shutdown_hooks.push(Box::new(move || {
            shutdown_token.cancel();
        }));

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
        C::register_meta(&mut self.meta_registry);
        self.consumer_registrations
            .push(Box::new(|state| C::register_consumers(state)));

        // Auto-validate config keys and sections declared on this controller
        if let Some(config) = &self.shared.config {
            let errors = C::validate_config(config);
            if !errors.is_empty() {
                let err = crate::config::ConfigValidationError { errors };
                panic!(
                    "\n=== CONFIGURATION ERRORS (controller: {}) ===\n\n{}\n============================\n",
                    std::any::type_name::<C>(),
                    err
                );
            }
        }

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

    /// Register a typed metadata consumer.
    ///
    /// At `build()` time, the consumer receives a shared slice of all `M` items
    /// from the [`MetaRegistry`] and returns a `Router<T>` to merge into the app.
    /// Multiple consumers for the same type can coexist (non-draining).
    ///
    /// # Example
    ///
    /// ```ignore
    /// app.with_meta_consumer::<RouteInfo, _>(|items| {
    ///     openapi_routes::<T>(config, items)
    /// })
    /// ```
    pub fn with_meta_consumer<M, F>(mut self, f: F) -> Self
    where
        M: Any + Send + Sync,
        F: FnOnce(&[M]) -> crate::http::Router<T> + Send + 'static,
    {
        self.meta_consumers.push(Box::new(move |registry| {
            let items = registry.get_or_empty::<M>();
            f(items)
        }));
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

        // Invoke meta consumers (e.g. OpenAPI spec builder).
        let meta_registry = self.meta_registry;
        for consumer in self.meta_consumers {
            let consumer_router = consumer(&meta_registry);
            router = router.merge(consumer_router);
        }

        // Apply the application state.
        let mut app = router.with_state(state.clone());

        // Install trailing-slash normalization fallback.
        // When no route matches and the path has a trailing slash, strip it
        // and re-dispatch to the same router via `tower::ServiceExt::oneshot`.
        if self.shared.normalize_path {
            use crate::http::response::IntoResponse;
            let inner = app.clone();
            app = app.fallback(move |req: crate::http::extract::Request| async move {
                let path = req.uri().path();
                if path.len() > 1 && path.ends_with('/') {
                    let trimmed = path.trim_end_matches('/');
                    let new_uri = match req.uri().query() {
                        Some(q) => format!("{}?{}", trimmed, q),
                        None => trimmed.to_string(),
                    };
                    let (mut parts, body) = req.into_parts();
                    parts.uri = new_uri.parse().unwrap_or(parts.uri);
                    let new_req = crate::http::header::HttpRequest::from_parts(parts, body);
                    match tower::ServiceExt::oneshot(inner.clone(), new_req).await {
                        Ok(resp) => resp,
                        Err(infallible) => match infallible {},
                    }
                } else {
                    crate::http::StatusCode::NOT_FOUND.into_response()
                }
            });
        }

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
        // Serve hooks should run even if no scheduler is installed.
        let mut boxed_tasks = plugin_data
            .get(&TypeId::of::<TaskRegistryHandle>())
            .and_then(|d| d.downcast_ref::<TaskRegistryHandle>())
            .map(|registry| registry.take_all())
            .unwrap_or_default();
        for hook in serve_hooks {
            // Tasks are single-consumer; only the first hook receives them.
            let tasks_for_hook = if boxed_tasks.is_empty() {
                Vec::new()
            } else {
                std::mem::take(&mut boxed_tasks)
            };
            // The token is captured in the hook closure
            hook(tasks_for_hook, CancellationToken::new());
        }

        // Run startup hooks
        for hook in startup_hooks {
            hook(state.clone())
                .await
                .map_err(|e| -> Box<dyn std::error::Error> { e })?;
        }

        let listener = tokio::net::TcpListener::bind(addr).await?;
        info!(%addr, "R2E server listening");
        crate::http::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
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

        info!("R2E server stopped");
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
