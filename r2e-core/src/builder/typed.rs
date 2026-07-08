//! Typed phase of [`AppBuilder`]: controllers, plugins, layers, lifecycle
//! hooks, and assembly (`build()` / `prepare()` / `serve()`).

use super::*;

// ── Typed phase (state resolved) ────────────────────────────────────────────

impl<T: Clone + Send + Sync + 'static> AppBuilder<T> {
    /// Internal: construct a typed builder from the pre-state shared config.
    ///
    /// `bean_context` is the resolved bean graph (retained so controllers and
    /// background services can be constructed by type); the `with_state` path
    /// passes an empty context.
    pub(super) fn from_pre(
        mut shared: BuilderConfig,
        state: T,
        bean_context: Arc<crate::beans::BeanContext>,
    ) -> Self {
        // Take the deferred actions before creating the builder.
        let deferred_actions = std::mem::take(&mut shared.deferred_actions);

        // Drop the bean registry since it's been consumed.
        shared.bean_registry = BeanRegistry::new();

        let mut builder = Self {
            shared,
            state,
            bean_context,
            routes: Vec::new(),
            startup_hooks: Vec::new(),
            shutdown_hooks: Vec::new(),
            meta_registry: MetaRegistry::new(),
            meta_consumers: Vec::new(),
            consumer_registrations: Vec::new(),
            serve_hooks: Vec::new(),
            plugin_shutdown_hooks: Vec::new(),
            plugin_async_shutdown_hooks: Vec::new(),
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
                async_shutdown_hooks: &mut builder.plugin_async_shutdown_hooks,
            };
            (action.action)(&mut ctx);
        }

        builder
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

    /// Returns a reference to the loaded [`R2eConfig`], if any.
    ///
    /// This is available after [`load_config()`](AppBuilder::load_config) or
    /// [`with_config()`](AppBuilder::with_config) has been called.
    pub fn r2e_config(&self) -> Option<&crate::config::R2eConfig> {
        self.shared.config.as_ref()
    }

    /// The application state.
    ///
    /// After [`build_state`](AppBuilder::build_state) this is the HList of
    /// resolved beans; read individual beans with `state().get::<T>()`
    /// (see [`BeanAccess`](crate::type_list::BeanAccess)).
    pub fn state(&self) -> &T {
        &self.state
    }

    /// The resolved bean graph, retained through the typed phase.
    ///
    /// Controller cores and background services are constructed from this
    /// context by type. Empty on the [`with_state`](AppBuilder::with_state)
    /// path.
    pub fn bean_context(&self) -> &Arc<crate::beans::BeanContext> {
        &self.bean_context
    }

    /// Whether the DevReload plugin has already been applied.
    pub(crate) fn is_dev_reload_applied(&self) -> bool {
        self.shared.dev_reload_applied
    }

    /// Mark the DevReload plugin as applied (prevents double-install).
    pub(crate) fn mark_dev_reload_applied(&mut self) {
        self.shared.dev_reload_applied = true;
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
    ///     .build_state()
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
    /// The hook receives the application state, mirroring [`on_start`](Self::on_start).
    ///
    /// # Example
    ///
    /// ```ignore
    /// AppBuilder::new()
    ///     .on_stop(|_state| async { tracing::info!("Bye"); })
    /// ```
    pub fn on_stop<F, Fut>(mut self, hook: F) -> Self
    where
        F: FnOnce(T) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        self.shutdown_hooks
            .push(Box::new(move |state| Box::pin(hook(state))));
        self
    }

    /// Set a maximum grace period for shutdown.
    ///
    /// When a shutdown signal is received the server stops accepting new
    /// connections and runs plugin/user shutdown hooks. If those hooks do
    /// not complete within `duration` the process will force-exit.
    ///
    /// By default there is **no** grace period — the process waits
    /// indefinitely for hooks to finish.
    ///
    /// # Example
    ///
    /// ```ignore
    /// AppBuilder::new()
    ///     .shutdown_grace_period(Duration::from_secs(5))
    ///     .serve("0.0.0.0:3000").await
    /// ```
    pub fn shutdown_grace_period(mut self, duration: Duration) -> Self {
        self.shared.shutdown_grace_period = Some(duration);
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
    /// The service is constructed from the retained bean graph via
    /// [`ServiceComponent::from_context`] and started in a Tokio task during
    /// `on_start`. A [`CancellationToken`] is provided and cancelled
    /// automatically during shutdown.
    ///
    /// # Example
    ///
    /// ```ignore
    /// AppBuilder::new()
    ///     .provide(pool)
    ///     .build_state().await
    ///     .spawn_service::<MetricsExporter>()
    ///     .serve("0.0.0.0:3000").await
    /// ```
    pub fn spawn_service<C: ServiceComponent>(mut self) -> Self {
        let token = CancellationToken::new();
        let shutdown_token = token.clone();

        // Get-or-insert the shared ServiceHandles collector in plugin_data so
        // `run_with_listener` can await all spawn_service tasks on shutdown.
        let handles = self
            .shared
            .plugin_data
            .entry(TypeId::of::<ServiceHandles>())
            .or_insert_with(|| Box::new(ServiceHandles::default()))
            .downcast_ref::<ServiceHandles>()
            .expect("ServiceHandles type mismatch in plugin_data")
            .clone();

        let service = C::from_context(&self.bean_context);
        self = self.on_start(move |_state| async move {
            let join = crate::rt::spawn(service.start(token));
            handles.push(join);
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
    /// [`DeferredContext::store_data`].
    pub fn get_plugin_data<D: Any + Send + Sync + 'static>(&self) -> Option<&D> {
        self.shared
            .plugin_data
            .get(&TypeId::of::<D>())
            .and_then(|boxed| boxed.downcast_ref::<D>())
    }

    /// Registration backend with all witnesses explicit: the public face is
    /// [`RegisterController::register_controller`](super::RegisterController::register_controller),
    /// which infers `W` (extraction markers) and `DepIdx` (dependency indices).
    ///
    /// # Panics
    ///
    /// Panics if config keys or sections declared on the controller fail
    /// validation.
    #[doc(hidden)]
    pub fn register_controller_impl<C, W, DepIdx>(self) -> Self
    where
        C: Controller<T, W>,
        C::Deps: crate::type_list::AllSatisfied<T, DepIdx>,
    {
        self.try_register_controller_impl::<C, W, DepIdx>()
            .unwrap_or_else(|err| {
                panic!(
                    "\n=== CONFIGURATION ERRORS (controller: {}) ===\n\n{}\n============================\n",
                    std::any::type_name::<C>(),
                    err
                )
            })
    }

    /// Non-panicking registration backend; see
    /// [`register_controller_impl`](Self::register_controller_impl).
    #[doc(hidden)]
    pub fn try_register_controller_impl<C, W, DepIdx>(
        mut self,
    ) -> Result<Self, crate::config::ConfigValidationError>
    where
        C: Controller<T, W>,
        C::Deps: crate::type_list::AllSatisfied<T, DepIdx>,
    {
        C::register_meta(&mut self.meta_registry);

        // Auto-validate config keys and sections declared on this controller
        if let Some(config) = &self.shared.config {
            let errors = C::validate_config(config);
            if !errors.is_empty() {
                return Err(crate::config::ConfigValidationError { errors });
            }
        }

        // Construct and bind app-scoped controllers only after config
        // validation, so configuration errors retain their aggregated report.
        // State-generic controllers construct from the retained bean context
        // (by type); named-state controllers read the typed state.
        let state = &self.state;
        let core = Arc::new(C::construct(state, &self.bean_context));
        self.routes.push(C::routes(state, Arc::clone(&core)));

        // Collect scheduled tasks (type-erased) and add to the task registry if present.
        // Tasks capture the state, so we need to pass it here.
        {
            let boxed_tasks = C::scheduled_tasks_boxed(&self.state, Arc::clone(&core));
            if !boxed_tasks.is_empty() {
                if let Some(registry) = self.get_plugin_data::<TaskRegistryHandle>() {
                    registry.add_boxed_for::<ScheduledTaskMarker>(boxed_tasks);
                } else {
                    tracing::warn!(
                        controller = std::any::type_name::<C>(),
                        "Scheduled tasks found but no scheduler installed. \
                         Add `.with_plugin(Scheduler)` before build_state()."
                    );
                }
            }
        }

        // Consumers start later during serve(), but use the same controller
        // core that was constructed above for routes and scheduled tasks.
        self.consumer_registrations.push(Box::new(move |state| {
            C::register_consumers(state, core)
        }));

        Ok(self)
    }

    /// Register a bean's event subscriptions.
    ///
    /// The bean is pulled from the retained bean graph by type and its
    /// [`EventSubscriber::subscribe()`] method is called during server startup.
    ///
    /// # Panics
    ///
    /// Panics if `S` was not provided/registered on the builder before
    /// `build_state()`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// AppBuilder::new()
    ///     .register::<NotificationService>()
    ///     .build_state().await
    ///     .register_subscriber::<NotificationService>()
    ///     .serve("0.0.0.0:3000").await.unwrap();
    /// ```
    pub fn register_subscriber<S>(mut self) -> Self
    where
        S: crate::EventSubscriber + Clone + 'static,
    {
        let subscriber = self.bean_context.try_get::<S>().unwrap_or_else(|| {
            panic!(
                "register_subscriber::<{ty}>(): bean not found in the resolved graph — \
                 add `.register::<{ty}>()` or `.provide(...)` before `build_state()`",
                ty = std::any::type_name::<S>()
            )
        });
        self.consumer_registrations.push(Box::new(move |_state| {
            subscriber.subscribe()
        }));
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
    pub fn build(self) -> crate::http::Router {
        self.build_inner().router
    }

    fn build_inner(self) -> BuiltApp<T> {
        let state = self.state;

        let mut router = crate::http::Router::new();

        // Merge all controller / manual routes.
        for r in self.routes {
            router = router.merge(r);
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

        // Always install the CatchPanicLayer as the outermost layer so that
        // panics anywhere in the stack are caught and turned into JSON 500
        // responses instead of crashing the process.
        app = app.layer(crate::layers::catch_panic_layer());

        BuiltApp {
            router: app,
            startup_hooks: self.startup_hooks,
            shutdown_hooks: self.shutdown_hooks,
            consumer_registrations: self.consumer_registrations,
            serve_hooks: self.serve_hooks,
            plugin_shutdown_hooks: self.plugin_shutdown_hooks,
            plugin_async_shutdown_hooks: self.plugin_async_shutdown_hooks,
            plugin_data: self.shared.plugin_data,
            state,
            shutdown_grace_period: self.shared.shutdown_grace_period,
        }
    }

    /// Build the application without starting the server.
    ///
    /// Returns a [`PreparedApp`] that holds the assembled router, state,
    /// hooks, and address. Call [`.run()`](PreparedApp::run) on it to
    /// start listening, or inspect the router for testing.
    ///
    /// Separating preparation from serving enables hot-reload:
    /// - `prepare()` can be called inside the hot-patched closure
    /// - The setup that produces beans/config stays outside
    pub fn prepare(self, addr: &str) -> PreparedApp<T> {
        #[cfg(feature = "dev-reload")]
        let this = if !self.shared.dev_reload_applied {
            self.with(crate::plugins::DevReload)
        } else {
            self
        };
        #[cfg(not(feature = "dev-reload"))]
        let this = self;

        #[cfg(feature = "quic")]
        let quic_server_config = this.shared.config.as_ref().and_then(|config| {
            let port = config.try_get::<u16>("server.quic.port")?;
            let cert_path = config.try_get::<String>("server.quic.cert")
                .or_else(|| {
                    tracing::error!("server.quic.port is set but server.quic.cert is missing");
                    None
                })?;
            let key_path = config.try_get::<String>("server.quic.key")
                .or_else(|| {
                    tracing::error!("server.quic.port is set but server.quic.key is missing");
                    None
                })?;
            let host = config
                .try_get::<String>("server.host")
                .unwrap_or_else(|| "0.0.0.0".into());
            let addr_str = format!("{host}:{port}");
            let bind_addr: std::net::SocketAddr = addr_str.parse().ok().or_else(|| {
                tracing::error!(addr = %addr_str, "Invalid QUIC bind address");
                None
            })?;
            match crate::http::quic::build_server_config_from_files(&cert_path, &key_path) {
                Ok(server_config) => Some((bind_addr, server_config)),
                Err(e) => {
                    tracing::error!(error = %e, "Failed to load QUIC TLS config");
                    None
                }
            }
        });

        #[cfg(feature = "quic")]
        let quic_alt_svc_max_age = this.shared.config.as_ref()
            .and_then(|c| c.try_get::<u32>("server.quic.alt_svc_max_age"))
            .unwrap_or(3600);

        let tcp_nodelay = this.shared.config.as_ref()
            .and_then(|c| c.try_get::<bool>("server.tcp_nodelay"))
            .unwrap_or(true);

        // Parse `server.workers` (SO_REUSEPORT sharding). Parsing happens here
        // (like `tcp_nodelay`) but `prepare()` is infallible, so the result —
        // including parse errors for invalid values like 0 or unknown strings —
        // is carried on `PreparedApp` and surfaced at `run()` time.
        let workers = crate::sharded::parse_workers(this.shared.config.as_ref());

        let BuiltApp {
            router,
            startup_hooks,
            shutdown_hooks,
            consumer_registrations,
            serve_hooks,
            plugin_shutdown_hooks,
            plugin_async_shutdown_hooks,
            plugin_data,
            state,
            shutdown_grace_period,
        } = this.build_inner();

        #[cfg(feature = "quic")]
        let router = if let Some((ref quic_addr, _)) = quic_server_config {
            crate::http::quic::apply_alt_svc(router, quic_addr.port(), quic_alt_svc_max_age)
        } else {
            router
        };

        PreparedApp {
            router,
            state,
            addr: addr.to_string(),
            startup_hooks,
            shutdown_hooks,
            consumer_registrations,
            serve_hooks,
            plugin_shutdown_hooks,
            plugin_async_shutdown_hooks,
            plugin_data,
            shutdown_grace_period,
            tcp_nodelay,
            workers,
            #[cfg(feature = "quic")]
            quic_server_config,
        }
    }

    /// Build the application and start serving on the given address.
    ///
    /// Runs startup hooks before listening, and shutdown hooks after
    /// graceful shutdown completes. Equivalent to `.prepare(addr).run().await`.
    pub async fn serve(self, addr: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.prepare(addr).run().await
    }

    /// Build the application and start serving, reading `server.host` and
    /// `server.port` from the configuration.
    ///
    /// Falls back to `0.0.0.0:3000` when no config is loaded or the keys
    /// are absent.
    pub async fn serve_auto(self) -> Result<(), Box<dyn std::error::Error>> {
        let addr = match &self.shared.config {
            Some(config) => {
                let host = config
                    .get::<String>("server.host")
                    .unwrap_or_else(|_| "0.0.0.0".into());
                let port = config.get::<u16>("server.port").unwrap_or(3000);
                format!("{host}:{port}")
            }
            None => "0.0.0.0:3000".into(),
        };
        self.prepare(&addr).run().await
    }
}

/// Output of [`AppBuilder::build_inner`]: the assembled router plus everything
/// the serving layer needs (hooks, state, plugin data).
///
/// Internal — `build()` keeps only the router, `prepare()` lifts the rest into
/// a [`PreparedApp`] together with the address and server tuning options.
struct BuiltApp<T: Clone + Send + Sync + 'static> {
    router: crate::http::Router,
    startup_hooks: Vec<StartupHook<T>>,
    shutdown_hooks: Vec<ShutdownHook<T>>,
    consumer_registrations: Vec<ConsumerReg<T>>,
    serve_hooks: Vec<ServeHook>,
    plugin_shutdown_hooks: Vec<Box<dyn FnOnce() + Send>>,
    plugin_async_shutdown_hooks: Vec<crate::plugin::AsyncShutdownHook>,
    plugin_data: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    state: T,
    shutdown_grace_period: Option<Duration>,
}
