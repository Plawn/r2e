//! Typed phase of [`AppBuilder`]: controllers, plugins, layers, lifecycle
//! hooks, and assembly (`build()` / `prepare()` / `serve()`).

use super::*;

// ── Typed phase (state resolved) ────────────────────────────────────────────

impl<T: Clone + Send + Sync + 'static> AppBuilder<T> {
    pub(crate) fn collect_service_sources(
        mut self,
        service_sources: Vec<(&'static str, crate::beans::ServiceSourceHook)>,
    ) -> Self {
        for (name, hook) in service_sources {
            let ctx = Arc::clone(&self.bean_context);
            self = self.register_service(name, move |token| {
                tracing::debug!(service = name, "started bean service");
                hook(&ctx, token)
            });
        }
        self
    }

    /// Spawn a background task, track its join handle for shutdown draining,
    /// and register a shutdown hook that cancels it. Shared by
    /// [`spawn_service`](Self::spawn_service) and
    /// [`collect_service_sources`](Self::collect_service_sources); `run`
    /// receives the [`CancelToken`] and returns the service future.
    ///
    /// `name` labels the tracked handle: it is what the `shutdown_grace_period`
    /// warning names when this service is the one that did not stop in time.
    fn register_service<F, Fut>(mut self, name: &'static str, run: F) -> Self
    where
        F: FnOnce(CancelToken) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        // A CHILD of the app shutdown root, not a fresh token. The sync
        // shutdown hook below cancels it early in the normal shutdown sequence
        // (before the HTTP drain, as documented), and cancelling the root
        // reaches it too — which is what covers the paths where no hook runs at
        // all: a panic unwinding out of `run_inner`, or the `run()` future
        // being dropped under an `r2e dev` hot patch. Liveness must not depend
        // on a hook firing.
        let token = shutdown_root(&mut self.shared.plugin_data).child_token();
        let shutdown_token = token.clone();

        // Get-or-insert the shared ServiceHandles collector in plugin_data so
        // `run_with_listener` can await all service tasks on shutdown.
        let handles = self
            .shared
            .plugin_data
            .entry(TypeId::of::<ServiceHandles>())
            .or_insert_with(|| Box::new(ServiceHandles::default()))
            .downcast_ref::<ServiceHandles>()
            .expect("ServiceHandles type mismatch in plugin_data")
            .clone();

        // The service task owns the graph while it runs: `spawn_service` tasks
        // are only *best-effort* awaited (an elapsed `shutdown_grace_period`,
        // or a dropped `run()` future under `r2e dev`, leaves them detached and
        // running), and a `BackgroundService` resolving through a `GraphHandle`
        // must not see a dead graph on those paths.
        let graph = Arc::clone(&self.bean_context);
        self = self.on_start(move |_state| async move {
            handles.spawn_owning(name, graph, run(token));
            Ok(())
        });
        self.plugin_shutdown_hooks.push(Box::new(move || {
            shutdown_token.cancel();
        }));
        self
    }

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
        // Take the deferred actions before creating the builder. The loaded
        // config is moved out too, so it can be lent to every `DeferredContext`
        // (plugins load their typed `Config` from it in `configure`).
        let deferred_actions = std::mem::take(&mut shared.deferred_actions);
        let deferred_config = shared.config.clone();
        // Pre-destroy disposers drained from the resolved graph at build_state().
        let bean_disposers = std::mem::take(&mut shared.bean_disposers);

        // Drop the bean registry since it's been consumed.
        shared.bean_registry = BeanRegistry::new();

        let mut builder = Self {
            shared,
            state,
            bean_context,
            routes: Vec::new(),
            startup_hooks: Vec::new(),
            shutdown_hooks: Vec::new(),
            drain_hooks: Vec::new(),
            meta_registry: MetaRegistry::new(),
            meta_consumers: Vec::new(),
            consumer_registrations: Vec::new(),
            post_construct_registrations: Vec::new(),
            on_start_hooks: Vec::new(),
            serve_hooks: Vec::new(),
            plugin_shutdown_hooks: Vec::new(),
            plugin_async_shutdown_hooks: Vec::new(),
            controller_disposers: Vec::new(),
            bean_disposers: Vec::new(),
            _provided: PhantomData,
            _required: PhantomData,
            _modules: PhantomData,
        };

        // Execute deferred actions (new API). They run here — after the bean
        // graph is resolved — so `ctx.bean_context()` exposes the fully
        // materialized graph (this is what backs plugin `configure`/`Deps`).
        for action in deferred_actions {
            let mut ctx = DeferredContext {
                layers: &mut builder.shared.custom_layers,
                router_wraps: &mut builder.shared.router_wraps,
                plugin_data: &mut builder.shared.plugin_data,
                serve_hooks: &mut builder.serve_hooks,
                shutdown_hooks: &mut builder.plugin_shutdown_hooks,
                async_shutdown_hooks: &mut builder.plugin_async_shutdown_hooks,
                bean_context: &builder.bean_context,
                config: deferred_config.as_ref(),
                routes_effects: &mut builder.shared.routes_effects,
                normalize_path: &mut builder.shared.normalize_path,
                dev_reload_applied: &mut builder.shared.dev_reload_applied,
            };
            (action.action)(&mut ctx);
        }

        // Bean pre-destroy disposers run within the async shutdown phase, at the
        // very end (after plugin async-shutdown hooks and controller disposers).
        // Reverse registration order among themselves was applied during
        // resolution. Held separately (not merged into the plugin hooks) so
        // controller `#[pre_destroy]` hooks can run *before* them.
        builder.bean_disposers = bean_disposers;

        builder
    }
}

impl<T: Clone + Send + Sync + 'static> AppBuilder<T> {
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

    /// Install the dev-reload endpoints and the `Cache-Control: no-store`
    /// layer, once. Used by `prepare()`'s automatic install; the
    /// [`DevReload`](crate::builtins::DevReload) plugin claims the same
    /// one-shot slot through
    /// [`DeferredContext::mark_dev_reload_applied`](crate::plugin::DeferredContext::mark_dev_reload_applied).
    #[cfg(feature = "dev-reload")]
    pub(crate) fn apply_dev_reload(mut self) -> Self {
        if self.shared.dev_reload_applied {
            return self;
        }
        self.shared.dev_reload_applied = true;
        self.register_routes(crate::runtime::dev::dev_routes())
            .with_layer_fn(|router| {
                router.layer(crate::http::middleware::from_fn(
                    crate::runtime::dev::dev_headers_middleware,
                ))
            })
    }

    // ── Layer primitives ────────────────────────────────────────────────

    /// Apply a Tower layer to the entire application.
    ///
    /// The layer is applied during `build()`. Multiple calls are applied in
    /// order. The layer must satisfy the same bounds as `Router::layer`.
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
    /// restrictive. The closure receives the `r2e::http::Router` and must
    /// return a new one.
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

    /// Register a drain hook, awaited when shutdown is triggered — **before**
    /// the server stops accepting connections.
    ///
    /// This is the place for "prepare the outside world for our departure"
    /// work: flip a readiness endpoint to unready, wait for the load balancer
    /// to deregister, broadcast a drain notice. The server keeps serving
    /// normally while drain hooks run; once all of them (and plugin shutdown
    /// hooks) complete, the listener stops accepting and in-flight requests
    /// finish. Compare [`on_stop`](Self::on_stop), which runs *after* the
    /// drain completes.
    ///
    /// # Example
    ///
    /// ```ignore
    /// AppBuilder::new()
    ///     .on_drain(|state| async move {
    ///         state.get::<Readiness>().set_draining();
    ///         r2e::rt::sleep(Duration::from_secs(5)).await; // LB deregistration
    ///     })
    /// ```
    pub fn on_drain<F, Fut>(mut self, hook: F) -> Self
    where
        F: FnOnce(T) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        self.drain_hooks
            .push(Box::new(move |state| Box::pin(hook(state))));
        self
    }

    /// Bound the **tracked-handle join** phase of shutdown, per handle.
    ///
    /// After the HTTP drain, tracked background tasks (`spawn_service`,
    /// gRPC/QUIC drains, [`ServeContext::track`](crate::builder::ServeContext::track))
    /// are joined. `duration` is the budget of **each handle separately**, and
    /// the handles are joined concurrently — one service ignoring its
    /// `CancelToken` is abandoned after `duration` with a warning naming it,
    /// and does not eat the budget of the others. The whole phase therefore
    /// takes at most `duration`, not `duration × services`.
    ///
    /// What it does **not** cover:
    ///
    /// | Phase | Bound by |
    /// |---|---|
    /// | [`on_drain`](Self::on_drain) hooks | nothing (they run before the drain) |
    /// | HTTP drain (in-flight requests) | [`drain_timeout`](Self::drain_timeout) |
    /// | tracked-handle join | **this**, per handle |
    /// | [`on_stop`](Self::on_stop) hooks | nothing — they **always** run |
    ///
    /// `on_stop` hooks used to share this budget and could be skipped entirely
    /// when a stuck service exhausted it. They no longer are: they carry
    /// application-state reconciliation (marking interrupted runs cancelled,
    /// releasing an advisory lock) and are treated as must-run.
    ///
    /// By default there is **no** grace period — shutdown waits indefinitely
    /// for tracked tasks.
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

    /// Bound the **HTTP drain**: how long in-flight requests may keep running
    /// after the listener has stopped accepting.
    ///
    /// The drain is bounded **by default** —
    /// [`DEFAULT_DRAIN_TIMEOUT`](crate::runtime::drain::DEFAULT_DRAIN_TIMEOUT)
    /// (30s, Spring's `timeout-per-shutdown-phase`). This method overrides that
    /// default, and wins over the `server.drain-timeout` config key.
    ///
    /// Unbounded is the plain-axum behavior and is reachable only on purpose,
    /// through [`drain_timeout_unbounded`](Self::drain_timeout_unbounded):
    /// with no bound, a single client holding a request — or an open SSE /
    /// streaming response — keeps the process alive forever, and
    /// `shutdown_grace_period` cannot help because it only starts once the
    /// drain is over.
    ///
    /// When the timeout elapses, a `warn!` names it and the remaining
    /// connections are abandoned (the serve future is dropped, closing them).
    /// Shutdown then continues normally: the tracked-handle join and the
    /// `on_stop` hooks still run.
    ///
    /// Applies identically to both serving strategies — under sharded serving
    /// (`server.workers`) each worker bounds its own drain, so the whole set
    /// still finishes within `duration` of the shutdown signal.
    ///
    /// # Example
    ///
    /// ```ignore
    /// AppBuilder::new()
    ///     .drain_timeout(Duration::from_secs(10))       // in-flight requests
    ///     .shutdown_grace_period(Duration::from_secs(5)) // background services
    ///     .serve("0.0.0.0:3000").await
    /// ```
    pub fn drain_timeout(mut self, duration: Duration) -> Self {
        self.shared.drain_timeout = Some(Some(duration));
        self
    }

    /// Opt out of the HTTP-drain bound entirely: wait for every in-flight
    /// request, however long it takes (the plain-axum behavior).
    ///
    /// This is the explicit escape hatch from the 30s default
    /// ([`DEFAULT_DRAIN_TIMEOUT`](crate::runtime::drain::DEFAULT_DRAIN_TIMEOUT)),
    /// for apps whose in-flight work must never be abandoned. It wins over the
    /// `server.drain-timeout` config key, and there is no config value that
    /// means "unbounded" — dropping the bound is a code decision.
    ///
    /// Be aware of what it costs: one client holding a request or a long-lived
    /// SSE/streaming response open keeps the process alive indefinitely, and
    /// no other shutdown budget can rescue it.
    pub fn drain_timeout_unbounded(mut self) -> Self {
        self.shared.drain_timeout = Some(None);
        self
    }

    /// Register a raw `r2e::http::Router` fragment to be merged into the application.
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

    /// Backend of the spawn path, without the dependency witness. The public
    /// face is [`SpawnService::spawn_service`](super::SpawnService::spawn_service),
    /// an extension trait so that `DepIdx` — the indices proving the service's
    /// `Deps` are all in the state — is inferred rather than turbofished
    /// (Rust forbids partial turbofish).
    ///
    /// # Panics
    ///
    /// Panics if config keys declared by the service are missing.
    pub(crate) fn spawn_service_impl<C: ServiceComponent>(self) -> Self {
        self.try_spawn_service_impl::<C>().unwrap_or_else(|err| {
            panic!(
                "\n=== CONFIGURATION ERRORS (service: {}) ===\n\n{}\n============================\n",
                std::any::type_name::<C>(),
                err
            )
        })
    }

    /// Non-panicking spawn backend; see
    /// [`spawn_service_impl`](Self::spawn_service_impl).
    ///
    /// The service's declared [`config_keys`](ServiceComponent::config_keys)
    /// and [`config_sections`](ServiceComponent::config_sections) are validated
    /// **before** `from_context` runs, so a missing `#[config]` key — or a
    /// missing/ill-typed key inside a `#[config_section]` — is an aggregated
    /// report naming every problem instead of a fail-late panic inside
    /// `R2eConfig::get` / `ConfigProperties::from_config`.
    pub(crate) fn try_spawn_service_impl<C: ServiceComponent>(
        self,
    ) -> Result<Self, crate::config::ConfigValidationError> {
        if let Some(config) = &self.shared.config {
            let mut errors = crate::config::validate_declared_keys(
                std::any::type_name::<C>(),
                &C::config_keys(),
                config,
            );
            errors.extend(crate::config::validate_declared_sections(
                &C::config_sections(),
                config,
            ));
            if !errors.is_empty() {
                return Err(crate::config::ConfigValidationError { errors });
            }
        }
        let service = C::from_context(&self.bean_context);
        Ok(self.register_service(std::any::type_name::<C>(), move |token| {
            service.start(token)
        }))
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
    pub(crate) fn register_controller_impl<C, W, DepIdx>(self) -> Self
    where
        C: Controller<T, W>,
        C::Deps: crate::type_list::AllSatisfied<T, DepIdx>,
    {
        self.register_controller_unchecked_impl::<C, W>()
    }

    /// Non-panicking registration backend; see
    /// [`register_controller_impl`](Self::register_controller_impl).
    pub(crate) fn try_register_controller_impl<C, W, DepIdx>(
        self,
    ) -> Result<Self, crate::config::ConfigValidationError>
    where
        C: Controller<T, W>,
        C::Deps: crate::type_list::AllSatisfied<T, DepIdx>,
    {
        self.try_register_controller_unchecked_impl::<C, W>()
    }

    /// Registration backend **without** the global dependency check.
    ///
    /// Used by the feature-module fold
    /// ([`ModuleList`](crate::di::module::ModuleList)): module controllers are
    /// dependency-checked module-locally at `register_module` (their deps may
    /// include private module beans, absent from the state); their cores
    /// construct from the retained bean context, where those beans exist.
    /// Everything else must go through the checked variants above.
    ///
    /// # Panics
    ///
    /// Panics if config keys or sections declared on the controller fail
    /// validation.
    pub(crate) fn register_controller_unchecked_impl<C, W>(self) -> Self
    where
        C: Controller<T, W>,
    {
        self.try_register_controller_unchecked_impl::<C, W>()
            .unwrap_or_else(|err| {
                panic!(
                    "\n=== CONFIGURATION ERRORS (controller: {}) ===\n\n{}\n============================\n",
                    std::any::type_name::<C>(),
                    err
                )
            })
    }

    /// Non-panicking variant of
    /// [`register_controller_unchecked_impl`](Self::register_controller_unchecked_impl).
    pub(crate) fn try_register_controller_unchecked_impl<C, W>(
        mut self,
    ) -> Result<Self, crate::config::ConfigValidationError>
    where
        C: Controller<T, W>,
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

        // Fill the core's decorator slot from the resolved graph — once, right
        // after construct, before scheduled tasks are built and before any
        // consumer or direct call can fire. No-op for controllers without
        // intercepted `#[scheduled]`/`#[consumer]` methods.
        C::fill_decos(&core, &self.bean_context);

        self.routes
            .push(C::routes(state, Arc::clone(&core), &self.bean_context));

        // Queue this core's `#[post_construct]` hooks — awaited at startup
        // before consumer registrations (no-op future for controllers without
        // `#[post_construct]`).
        self.post_construct_registrations
            .push(C::post_construct(Arc::clone(&core)));

        // Queue this core's `#[on_start]` hooks — merged with the bean hooks
        // into one order-sorted list and awaited at server startup, after the
        // consumer registrations and before the builder's `on_start` closures.
        // The default `Controller::on_start` returns an empty vec, so this is
        // free for controllers without the attribute.
        self.on_start_hooks.extend(C::on_start(Arc::clone(&core)));

        // Collect scheduled tasks (type-erased) and add to the task registry if present.
        // Tasks capture the state, so we need to pass it here.
        {
            let boxed_tasks =
                C::scheduled_tasks_boxed(&self.state, Arc::clone(&core), &self.bean_context);
            if !boxed_tasks.is_empty() {
                if let Some(registry) = self.get_plugin_data::<TaskRegistryHandle>() {
                    registry.add_boxed_for::<ScheduledTaskMarker>(boxed_tasks);
                } else {
                    tracing::warn!(
                        controller = std::any::type_name::<C>(),
                        "Scheduled tasks found but no scheduler installed. \
                         Add `.plugin(Scheduler)` before build_state()."
                    );
                }
            }
        }

        // Queue this core's `#[pre_destroy]` disposal hooks — awaited during the
        // async shutdown phase, before the bean disposers (a controller disposes
        // before the beans it injected). Pushed in registration order and
        // reversed once when the ordered async-shutdown list is assembled in
        // `build_inner`, so later-registered controllers dispose first. Skipped
        // entirely for controllers without a `#[pre_destroy]` hook (their
        // `pre_destroy` is the no-op default), avoiding a queued no-op per
        // controller.
        if C::HAS_PRE_DESTROY {
            let core_for_dispose = Arc::clone(&core);
            self.controller_disposers
                .push(Box::new(move || C::pre_destroy(core_for_dispose))
                    as crate::plugin::AsyncShutdownHook);
        }

        // Consumers start later during serve(), but use the same controller
        // core that was constructed above for routes and scheduled tasks.
        self.consumer_registrations
            .push(Box::new(move |state| C::register_consumers(state, core)));

        Ok(self)
    }

    /// Run the bean scheduled-source hooks (queued by `#[bean]` via
    /// `after_register` → `BeanRegistry::register_scheduled_source`) against
    /// the resolved graph and hand the collected tasks to the scheduler's
    /// task registry. Mirrors the controller path in
    /// [`try_register_controller_unchecked_impl`](Self::try_register_controller_unchecked_impl)
    /// (same marker tag, same missing-scheduler warning).
    ///
    /// Called by `build_state()` right after the typed builder exists — the
    /// deferred plugin actions have run by then, so the `Scheduler` plugin's
    /// `TaskRegistryHandle` is in the plugin data.
    pub(crate) fn collect_bean_scheduled_tasks(
        self,
        sources: Vec<(
            &'static str,
            Box<dyn FnOnce(&crate::beans::BeanContext) -> Vec<Box<dyn Any + Send>> + Send>,
        )>,
    ) -> Self {
        if sources.is_empty() {
            return self;
        }
        match self.get_plugin_data::<TaskRegistryHandle>() {
            Some(registry) => {
                for (_, hook) in sources {
                    registry.add_boxed_for::<ScheduledTaskMarker>(hook(&self.bean_context));
                }
            }
            None => {
                for (bean_name, _) in sources {
                    tracing::warn!(
                        bean = bean_name,
                        "Scheduled tasks found but no scheduler installed. \
                         Add `.plugin(Scheduler)` before build_state()."
                    );
                }
            }
        }
        self
    }

    /// Queue the bean `#[on_start]` hooks (registered by `#[bean]` via
    /// `after_register` → `BeanRegistry::register_on_start`), read from the
    /// resolved graph so pinned test overrides are honoured.
    ///
    /// Called by `build_state()` right after the typed builder exists —
    /// i.e. before controllers register, so bean hooks precede controller
    /// hooks at equal `order`.
    pub(crate) fn collect_bean_on_start(
        mut self,
        sources: Vec<(&'static str, crate::beans::OnStartSourceHook)>,
    ) -> Self {
        for (_, hook) in sources {
            self.on_start_hooks.extend(hook(&self.bean_context));
        }
        self
    }

    /// Queue the bean event-subscriber hooks (queued by `#[bean]` via
    /// `after_register` → `BeanRegistry::register_event_subscriber`) as
    /// consumer registrations, run at server startup (`serve` /
    /// [`build_with_consumers`](Self::build_with_consumers)) — the same point
    /// controller `#[consumer]` methods subscribe. Each hook reads its bean by
    /// type from the retained graph, so pinned test overrides are honoured.
    ///
    /// Called by `build_state()` right after the typed builder exists.
    pub(crate) fn collect_bean_subscribers(
        mut self,
        subscribers: Vec<(
            &'static str,
            Box<
                dyn FnOnce(
                        &crate::beans::BeanContext,
                    )
                        -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
                    + Send,
            >,
        )>,
    ) -> Self {
        for (_, hook) in subscribers {
            let ctx = Arc::clone(&self.bean_context);
            self.consumer_registrations
                .push(Box::new(move |_state| hook(&ctx)));
        }
        self
    }

    /// Register a raw consumer-registration hook, run once during server
    /// startup (at the same point controller and bean `#[consumer]` methods
    /// subscribe).
    ///
    /// This is the extension point downstream crates use to wire event
    /// subscriptions that aren't controller- or bean-shaped — e.g. the
    /// EventBus↔SSE bridge in `r2e-events`. Combine with
    /// [`bean_context`](Self::bean_context) to resolve the beans the hook
    /// needs.
    pub fn add_consumer_registration<F, Fut>(mut self, f: F) -> Self
    where
        F: FnOnce(T) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        self.consumer_registrations
            .push(Box::new(move |state| Box::pin(f(state))));
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

    /// Assemble the final `r2e::http::Router` from all registered routes and layers.
    ///
    /// Startup lifecycle work is NOT run here: consumer registrations AND
    /// controller `#[post_construct]` hooks are dropped. Use
    /// [`build_with_consumers`](Self::build_with_consumers) (or `serve`) when
    /// the app relies on either.
    pub fn build(self) -> crate::http::Router {
        self.build_inner().router
    }

    /// Like [`build`](Self::build), but also runs the consumer registrations
    /// (controller and bean `#[consumer]` methods,
    /// [`add_consumer_registration`](Self::add_consumer_registration)
    /// hooks) that `serve()` would run at startup.
    ///
    /// This is the in-process test entry point: it gives event consumers
    /// production parity without binding a listener. Serve hooks (scheduler
    /// task start, …) still do not run.
    pub async fn build_with_consumers(self) -> crate::http::Router {
        let built = self.build_inner();
        // Controller `#[post_construct]` runs before consumers (mirroring bean
        // post_construct at `build_state`, which runs before subscribers). This
        // entry point returns a `Router`, so an error fails loudly with a panic —
        // the same shape as `build_state` panicking on a bean post_construct Err.
        for pc in built.post_construct_registrations {
            pc.await
                .unwrap_or_else(|e| panic!("Controller #[post_construct] hook failed: {e}"));
        }
        for reg in built.consumer_registrations {
            reg(built.state.clone()).await;
        }
        // `#[on_start]` hooks DO run here (unlike `#[pre_destroy]`, which has no
        // shutdown to fire on): the graph and every controller core exist, which
        // is exactly the contract. `TestApp::boot` reaches this path, so tests
        // observe production startup behaviour. An `Err` panics, like the
        // controller `#[post_construct]` above.
        for (_, hook) in sort_on_start(built.on_start_hooks) {
            hook()
                .await
                .unwrap_or_else(|e| panic!("#[on_start] hook failed: {e}"));
        }
        built.router
    }

    fn build_inner(mut self) -> BuiltApp<T> {
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

        // ── Routes stage ────────────────────────────────────────────────
        // Every controller (app, module and plugin) has registered by now, so
        // a plugin effect queued with `after_routes` sees the complete route
        // metadata whatever the install order — this is what lets OpenAPI be a
        // plain `.plugin()` instead of "install me last". Routers a Routes
        // effect registers are merged BEFORE the custom layers below, so they
        // pick up the same middleware stack controller routes do.
        if !self.shared.routes_effects.is_empty() {
            let mut rctx = crate::plugin::RoutesContext::new(
                meta_registry.get_or_empty::<crate::di::meta::RouteInfo>(),
                &mut self.shared.plugin_data,
                &self.bean_context,
                self.shared.config.as_ref(),
            );
            for effect in std::mem::take(&mut self.shared.routes_effects) {
                effect(&mut rctx);
            }
            for r in rctx.into_routers() {
                app = app.merge(r);
            }
        }

        // Apply layers (in registration order). Layers added via
        // `Router::layer` run after routing, so they observe `MatchedPath`
        // (and any controller `#[fallback]`) on the routed request.
        for layer_fn in self.shared.custom_layers {
            app = layer_fn(app);
        }

        // Install trailing-slash normalization as a genuine pre-routing URI
        // rewrite: `/users/1/` is trimmed to `/users/1` BEFORE routing, so
        // the meaningful routing happens once and `MatchedPath` reaches every
        // layer applied above (metrics, tracing) — unlike a fallback
        // re-dispatch, which routes twice and hides the match from outer
        // instrumentation. See `layers::normalize_path_router` for the
        // wrap-and-re-embed mechanics and its caveats.
        if self.shared.normalize_path {
            app = crate::runtime::layers::normalize_path_router(app);
        }

        // Always install the CatchPanicLayer as the outermost HTTP layer so
        // that panics anywhere in the stack are caught and turned into JSON
        // 500 responses instead of crashing the process.
        app = app.layer(crate::runtime::layers::catch_panic_layer());

        // Transport-level wraps go outside EVERYTHING HTTP-shaped (custom
        // layers and catch-panic included): a multiplexer's non-HTTP branch
        // must never cross HTTP middleware — a JSON 500 is garbage to a gRPC
        // client. See `DeferredContext::wrap_router`.
        for wrap in self.shared.router_wraps {
            app = wrap(app);
        }

        // Hand the resolved bean graph to the router — OUTERMOST, so it covers
        // every route the assembly produced: controller routes, routes a plugin
        // mounted through `add_layer` (added *after* the state layer, so an
        // earlier install point would miss them), and whatever a transport wrap
        // dispatches to. Beans reach the graph through a WEAK `GraphHandle` (a
        // strong one would be a cycle that never frees anything — one leaked
        // graph per dev-reload cycle), so the router is what keeps it alive: the
        // `Arc` rides each request future and its response body, so a
        // `GraphHandle` inside a bean resolves for as long as anything derived
        // from this router is in flight, and the whole graph drops once nothing
        // is. Pure pass-through — see `layers::GraphKeepAlive`.
        app = app.layer(crate::runtime::layers::graph_keep_alive(Arc::clone(
            &self.bean_context,
        )));

        // Assemble the single ordered async-shutdown list, drained back-to-front
        // of the shutdown sequence: plugin async hooks, then controller
        // `#[pre_destroy]` hooks, then bean `#[pre_destroy]` disposers — so a
        // controller disposes before the beans it injected. Controller disposers
        // were pushed in registration order; reverse them here (the single
        // assembly point) so later-registered controllers dispose first. Bean
        // disposers already arrive in reverse registration order (applied during
        // graph resolution).
        let mut async_shutdown_hooks = self.plugin_async_shutdown_hooks;
        let mut controller_disposers = self.controller_disposers;
        controller_disposers.reverse();
        async_shutdown_hooks.extend(controller_disposers);
        async_shutdown_hooks.extend(self.bean_disposers);

        BuiltApp {
            router: app,
            startup_hooks: self.startup_hooks,
            shutdown_hooks: self.shutdown_hooks,
            drain_hooks: self.drain_hooks,
            consumer_registrations: self.consumer_registrations,
            post_construct_registrations: self.post_construct_registrations,
            on_start_hooks: self.on_start_hooks,
            serve_hooks: self.serve_hooks,
            plugin_shutdown_hooks: self.plugin_shutdown_hooks,
            async_shutdown_hooks,
            plugin_data: self.shared.plugin_data,
            state,
            shutdown_grace_period: self.shared.shutdown_grace_period,
            // Builder call > `server.drain-timeout` > 30s default.
            drain_timeout: crate::runtime::drain::resolve_drain_timeout(
                self.shared.drain_timeout,
                self.shared.config.as_ref(),
            ),
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
        let this = self.apply_dev_reload();
        #[cfg(not(feature = "dev-reload"))]
        let this = self;

        #[cfg(feature = "quic")]
        let quic_server_config = this.shared.config.as_ref().and_then(|config| {
            let port = config.try_get::<u16>("server.quic.port")?;
            let cert_path = config.try_get::<String>("server.quic.cert").or_else(|| {
                tracing::error!("server.quic.port is set but server.quic.cert is missing");
                None
            })?;
            let key_path = config.try_get::<String>("server.quic.key").or_else(|| {
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
        let quic_alt_svc_max_age = this
            .shared
            .config
            .as_ref()
            .and_then(|c| c.try_get::<u32>("server.quic.alt_svc_max_age"))
            .unwrap_or(3600);

        let tcp_nodelay = this
            .shared
            .config
            .as_ref()
            .and_then(|c| c.try_get::<bool>("server.tcp_nodelay"))
            .unwrap_or(true);

        // Parse `server.workers` (SO_REUSEPORT sharding). Parsing happens here
        // (like `tcp_nodelay`) but `prepare()` is infallible, so the result —
        // including parse errors for invalid values like 0 or unknown strings —
        // is carried on `PreparedApp` and surfaced at `run()` time.
        let workers = crate::runtime::sharded::parse_workers(this.shared.config.as_ref());
        let per_worker_services = this.shared.per_worker_services.clone();

        // Stop-handle resolution: explicit `with_stop_handle` wins, then a
        // `StopHandle` bean from the graph (so `.provide(stop.clone())` alone
        // is enough to wire an admin stop endpoint — a provided-but-unwired
        // handle would be a silent no-op), then a fresh handle.
        let stop_handle = this
            .shared
            .stop_handle
            .clone()
            .or_else(|| this.bean_context.try_get::<StopHandle>())
            .unwrap_or_default();

        // Serve-scope graph ownership: `PreparedApp` holds a strong reference
        // for the whole serving lifecycle, because the router (and its
        // `GraphKeepAlive` layer) is dropped as soon as the serve future
        // completes — before tracked handles and shutdown hooks run. See
        // `PreparedApp::graph`.
        let graph = Arc::clone(&this.bean_context);

        let BuiltApp {
            router,
            startup_hooks,
            shutdown_hooks,
            drain_hooks,
            consumer_registrations,
            post_construct_registrations,
            on_start_hooks,
            serve_hooks,
            plugin_shutdown_hooks,
            async_shutdown_hooks,
            plugin_data,
            state,
            shutdown_grace_period,
            drain_timeout,
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
            graph,
            addr: addr.to_string(),
            startup_hooks,
            shutdown_hooks,
            drain_hooks,
            stop_handle,
            consumer_registrations,
            post_construct_registrations,
            on_start_hooks,
            serve_hooks,
            plugin_shutdown_hooks,
            async_shutdown_hooks,
            plugin_data,
            shutdown_grace_period,
            drain_timeout,
            tcp_nodelay,
            workers,
            per_worker_services,
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
    drain_hooks: Vec<DrainHook<T>>,
    consumer_registrations: Vec<ConsumerReg<T>>,
    post_construct_registrations: Vec<PostConstructReg>,
    /// `#[on_start]` hooks from beans and controller cores, unsorted (sorted
    /// once at run time by [`sort_on_start`]).
    on_start_hooks: Vec<OnStartReg>,
    serve_hooks: Vec<ServeHook>,
    plugin_shutdown_hooks: Vec<Box<dyn FnOnce() + Send>>,
    /// Single ordered async-shutdown list: plugin async hooks ++ controller
    /// `#[pre_destroy]` hooks ++ bean `#[pre_destroy]` disposers. Assembled once
    /// in `build_inner` and drained in order during the async shutdown phase.
    async_shutdown_hooks: Vec<crate::plugin::AsyncShutdownHook>,
    plugin_data: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    state: T,
    shutdown_grace_period: Option<Duration>,
    drain_timeout: Option<Duration>,
}

/// Sort the collected `#[on_start]` hooks by their declared `order`, ascending.
///
/// `sort_by_key` is a **stable** sort, so hooks sharing an order keep
/// registration order: bean hooks (collected at `build_state()`) before
/// controller hooks (collected as controllers register), each group in
/// declaration order.
pub(super) fn sort_on_start(mut hooks: Vec<OnStartReg>) -> Vec<OnStartReg> {
    hooks.sort_by_key(|(order, _)| *order);
    hooks
}
