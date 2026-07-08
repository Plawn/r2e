//! NoState (pre-state) phase of [`AppBuilder`]: bean/producer registration,
//! config loading, pre-state plugins, and the `build_state` transition.

use super::*;

// ── NoState phase (pre-state) ───────────────────────────────────────────────

impl AppBuilder<NoState, TNil, TNil, TNil> {
    /// Create a new, empty builder in the pre-state phase.
    pub fn new() -> Self {
        Self {
            shared: BuilderConfig {
                config: None,
                custom_layers: Vec::new(),
                bean_registry: BeanRegistry::new(),
                deferred_actions: Vec::new(),
                plugin_data: HashMap::new(),
                last_plugin_name: None,
                normalize_path: false,
                dev_reload_applied: false,
                shutdown_grace_period: None,
                active_profile: "default".to_string(),
            },
            state: NoState,
            bean_context: Arc::new(crate::beans::BeanContext::empty()),
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
            _modules: PhantomData,
        }
    }
}

impl<P, R, Mods> AppBuilder<NoState, P, R, Mods> {
    /// Access the bean registry (for internal use by the blanket PreStatePlugin impl).
    pub(crate) fn bean_registry(&self) -> &BeanRegistry {
        &self.shared.bean_registry
    }

    /// Access the loaded config (for internal use by the blanket PreStatePlugin impl).
    ///
    /// Named differently from [`AppBuilder<T>::r2e_config()`] to avoid a method
    /// resolution conflict when `T = NoState`.
    pub(crate) fn r2e_config_ref(&self) -> Option<&crate::config::R2eConfig> {
        self.shared.config.as_ref()
    }

    /// Reconstruct the builder with updated type-level tracking parameters.
    ///
    /// Since `P` and `R` are phantom types used only for compile-time bean graph
    /// validation, this is a zero-cost conversion that just changes the markers.
    /// The pending-module list `Mods` is preserved.
    #[doc(hidden)]
    pub fn with_updated_types<NewP, NewR>(self) -> AppBuilder<NoState, NewP, NewR, Mods> {
        self.with_updated_types_full()
    }

    /// [`with_updated_types`](Self::with_updated_types), but also rewriting
    /// the pending-module list. Internal — only `register_module` grows `Mods`.
    fn with_updated_types_full<NewP, NewR, NewMods>(
        self,
    ) -> AppBuilder<NoState, NewP, NewR, NewMods> {
        AppBuilder {
            shared: self.shared,
            state: NoState,
            bean_context: self.bean_context,
            routes: self.routes,
            startup_hooks: self.startup_hooks,
            shutdown_hooks: self.shutdown_hooks,
            meta_registry: self.meta_registry,
            meta_consumers: self.meta_consumers,
            consumer_registrations: self.consumer_registrations,
            serve_hooks: self.serve_hooks,
            plugin_shutdown_hooks: self.plugin_shutdown_hooks,
            plugin_async_shutdown_hooks: self.plugin_async_shutdown_hooks,
            _provided: PhantomData,
            _required: PhantomData,
            _modules: PhantomData,
        }
    }

    /// Provide a pre-built bean instance.
    ///
    /// The instance will be available in the [`BeanContext`](crate::beans::BeanContext)
    /// for beans that depend on type `B`, and will be pulled into the state
    /// struct when [`build_state`](Self::build_state) is called.
    pub fn provide<B: Clone + Send + Sync + 'static>(mut self, bean: B) -> AppBuilder<NoState, TCons<B, P>, R, Mods> {
        self.shared.bean_registry.provide(bean);
        self.with_updated_types()
    }

    /// Register a bean, async bean, or producer for automatic construction.
    ///
    /// Unified entry point over the three registration kinds. The type must
    /// implement [`Registrable`], which is emitted automatically by `#[bean]`,
    /// `#[derive(Bean)]`, and `#[producer]`:
    ///
    /// - `#[bean]` (sync) / `#[derive(Bean)]` register a sync bean.
    /// - `#[bean]` (async) registers an async bean, awaited during
    ///   [`build_state`](Self::build_state).
    /// - `#[producer]` registers the producer's output type (not the producer
    ///   struct) in the provision list.
    ///
    /// The provided type (`T::Provided`) is tracked in the compile-time
    /// provision list and its dependencies (`T::Deps`) are appended to the
    /// requirement list.
    pub fn register<T: Registrable>(mut self) -> Registered<T::Provided, T::Deps, P, R, Mods>
    where
        R: TAppend<T::Deps>,
    {
        T::register_into(&mut self.shared.bean_registry);
        self.with_updated_types()
    }

    /// Check whether a config key is truthy (a boolean `true`).
    ///
    /// Requires [`load_config`](Self::load_config) or
    /// [`with_config`](Self::with_config) to have been called first. Combine it
    /// with [`when`](Self::when) for conditional assembly, or use it to compute
    /// the flag a `#[producer] -> Option<T>` keys off:
    ///
    /// ```ignore
    /// let b = AppBuilder::new().load_config::<AppConfig>();
    /// let enabled = b.config_flag("features.cache");
    /// b.provide(CacheEnabled(enabled)).register::<CacheProducer>()
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if no config has been loaded.
    pub fn config_flag(&self, key: &str) -> bool {
        self.shared
            .config
            .as_ref()
            .expect("config_flag requires config — call .load_config() or .with_config() first")
            .try_get::<bool>(key)
            .unwrap_or(false)
    }

    // ── Profile inspection ─────────────────────────────────────────────

    /// Returns the active profile name.
    ///
    /// Resolved (in priority order) from:
    /// 1. `R2E_PROFILE` environment variable
    /// 2. `r2e.profile` config key
    /// 3. `"default"` (fallback)
    ///
    /// The profile is set when [`load_config`](Self::load_config) or
    /// [`with_config`](Self::with_config) is called. Before that, it is
    /// `"default"`.
    pub fn active_profile(&self) -> &str {
        &self.shared.active_profile
    }

    /// Returns `true` if the active profile matches `profile`.
    ///
    /// Convenience over [`active_profile`](Self::active_profile) for use with
    /// [`when`](Self::when) or to compute a flag a `#[producer] -> Option<T>`
    /// keys off.
    pub fn profile_is(&self, profile: &str) -> bool {
        self.shared.active_profile == profile
    }

    // ── Default bean registration (last-wins override) ─────────────────

    /// Register a default bean that a later registration can override.
    ///
    /// The bean IS added to the provision list (guaranteed to be present). A
    /// later [`register`](Self::register) of the same type silently replaces
    /// this registration (last-wins), without changing the provision list.
    pub fn with_default_bean<B: Bean>(mut self) -> Registered<B, B::Deps, P, R, Mods>
    where
        R: TAppend<B::Deps>,
    {
        self.shared.bean_registry.register_default::<B>();
        self.with_updated_types()
    }

    /// Register a default async bean that can be overridden by alternatives.
    ///
    /// The bean IS added to the provision list (guaranteed to be present).
    pub fn with_default_async_bean<B: AsyncBean>(mut self) -> Registered<B, B::Deps, P, R, Mods>
    where
        R: TAppend<B::Deps>,
    {
        self.shared.bean_registry.register_async_default::<B>();
        self.with_updated_types()
    }

    /// Register a default producer that can be overridden by alternatives.
    ///
    /// The producer's output IS added to the provision list (guaranteed to be present).
    pub fn with_default_producer<Pr: Producer>(mut self) -> Registered<Pr::Output, Pr::Deps, P, R, Mods>
    where
        R: TAppend<Pr::Deps>,
    {
        self.shared.bean_registry.register_producer_default::<Pr>();
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
    ///     .build_state().await
    /// ```
    pub fn with_bean_factory<B, F>(
        mut self,
        factory: F,
    ) -> Registered<B, TCons<crate::config::R2eConfig, TNil>, P, R, Mods>
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

    /// Provide a pre-loaded configuration to the builder.
    ///
    /// Stores the config and provides `R2eConfig` in the bean registry
    /// (injectable by beans and controllers).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let config = R2eConfig::load()?;
    /// AppBuilder::new()
    ///     .with_config(config)
    ///     .build_state()
    ///     .await
    /// ```
    pub fn with_config(
        mut self,
        config: crate::config::R2eConfig,
    ) -> AppBuilder<NoState, TCons<crate::config::R2eConfig, P>, R, Mods> {
        self.shared.active_profile = resolve_profile(&config);
        self.shared.config = Some(config.clone());
        self.shared.bean_registry.provide(config);
        self.with_updated_types()
    }

    /// Load configuration and provide it to the builder.
    ///
    /// Combines loading, optional typed construction, and registration in one call:
    /// 1. Loads `application.yaml` + `.env` + env var overlay
    /// 2. If `C` implements [`ConfigProperties`](crate::config::ConfigProperties),
    ///    constructs the typed config and provides it as a bean
    /// 3. Stores the raw config + provides `R2eConfig` in the bean registry
    ///
    /// # Panics
    ///
    /// Panics if configuration loading or typed construction fails.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // Raw config only:
    /// AppBuilder::new()
    ///     .load_config::<()>()
    ///
    /// // With typed config struct:
    /// AppBuilder::new()
    ///     .load_config::<AppConfig>()
    /// ```
    pub fn load_config<C: crate::config::LoadableConfig>(
        mut self,
    ) -> WithLoadedConfig<C, P, R, Mods>
    where
        C::Children: TAppend<P>,
    {
        let config = crate::config::R2eConfig::load()
            .unwrap_or_else(|e| panic!("Failed to load config: {e}"));
        C::register(&config, &mut self.shared.bean_registry)
            .unwrap_or_else(|e| panic!("Failed to construct typed config: {e}"));
        self.shared.active_profile = resolve_profile(&config);
        self.shared.config = Some(config.clone());
        self.shared.bean_registry.provide(config);
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
    ///     .build_state()
    ///     .await
    /// ```
    pub fn plugin<Pl: RawPreStatePlugin, RIdx>(
        self,
        plugin: Pl,
    ) -> WithPluginInstalled<Pl, P, R, Mods>
    where
        P: TAppend<Pl::Provisions>,
        R: TAppend<Pl::Required>,
        Pl::Required: AllSatisfied<P, RIdx>,
    {
        plugin.install(self)
    }

    /// Alias for [`.plugin()`](Self::plugin) for pre-state plugins.
    ///
    /// # Deprecated
    ///
    /// Use [`.plugin()`](Self::plugin) instead.
    #[deprecated(since = "0.2.0", note = "Use .plugin() instead")]
    pub fn with_plugin<Pl: RawPreStatePlugin, RIdx>(
        self,
        plugin: Pl,
    ) -> WithPluginInstalled<Pl, P, R, Mods>
    where
        P: TAppend<Pl::Provisions>,
        R: TAppend<Pl::Required>,
        Pl::Required: AllSatisfied<P, RIdx>,
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
    pub fn add_deferred(mut self, action: DeferredAction) -> Self {
        self.shared.deferred_actions.push(action);
        self
    }

    /// Register a bean, async bean, or producer as an **override** of an
    /// existing registration of the same provided type — typically one added
    /// via [`with_default_bean`](Self::with_default_bean) /
    /// [`with_default_async_bean`](Self::with_default_async_bean) /
    /// [`with_default_producer`](Self::with_default_producer).
    ///
    /// Unlike [`register`](Self::register), this does **not** push the type
    /// onto the compile-time provision list `P` — the default registration
    /// already guarantees presence, and a duplicate slot would make
    /// `state.get::<T>()` ambiguous. The registry resolves the pair with
    /// last-wins semantics (the default is dropped).
    pub fn register_override<T: Registrable>(mut self) -> Self {
        T::register_into(&mut self.shared.bean_registry);
        self
    }

    /// Registration backend for feature modules, with all witnesses explicit:
    /// the public face is
    /// [`RegisterModule::register_module`](super::RegisterModule::register_module),
    /// which infers them.
    pub(crate) fn register_module_impl<M, DepIdx, ExpIdx, CtrlIdx>(
        mut self,
    ) -> ModuleRegistered<M, P, R, Mods>
    where
        M: FeatureModule,
        M::Providers: BeanList,
        <M::Providers as BeanList>::Provided: TAppend<M::Imports>,
        M::Controllers: ControllerDepsList,
        // Encapsulation: provider deps, exports, and controller deps must
        // resolve inside the module scope (Provided ∪ Imports).
        <M::Providers as BeanList>::Deps: ModuleDepsSatisfied<ModuleScope<M>, DepIdx>,
        M::Exports: ExportsProvided<<M::Providers as BeanList>::Provided, ExpIdx>,
        <M::Controllers as ControllerDepsList>::Deps: ModuleDepsSatisfied<ModuleScope<M>, CtrlIdx>,
        M::Exports: TAppend<P>,
        R: TAppend<M::Imports>,
    {
        <M::Providers as BeanList>::register_into(&mut self.shared.bean_registry);
        // Exports join P, imports join R, and M is queued on Mods. Provider
        // -internal deps are consumed by the module-local check above — they
        // must NOT join the global R (private providers are absent from P).
        self.with_updated_types_full()
    }

    /// Resolve the bean dependency graph and build the application state.
    ///
    /// Consumes the bean registry, topologically sorts all beans, constructs
    /// them in order (awaiting async beans/producers), then materializes the
    /// state: a type-level HList mirroring the provision list `P`, with one
    /// resolved bean instance per slot (see
    /// [`BuildHList`](crate::type_list::BuildHList)). The state type is fully
    /// inferred from the builder chain — there is no hand-written state struct.
    ///
    /// Beans are read out of the state by type via
    /// [`BeanAccess::get`](crate::type_list::BeanAccess::get)
    /// (`state.get::<T>()`), which monomorphizes to a fixed-offset field
    /// access.
    ///
    /// The `R` (requirements) list is checked against `P` (provisions) at
    /// compile time via the [`AllSatisfied`] bound: every bean dependency must
    /// be present in the provision list, or the compiler emits an error.
    ///
    /// Note: apps with more than ~127 registrations may need
    /// `#![recursion_limit = "512"]` at the crate root (the index-witness
    /// chains exceed rustc's default recursion limit).
    ///
    /// # Panics
    ///
    /// Panics if the bean graph has cycles, missing dependencies, or duplicate
    /// registrations. Use [`try_build_state`](Self::try_build_state) for a
    /// non-panicking alternative.
    pub async fn build_state<W, MW>(self) -> AppBuilder<<P as BuildHList>::Output>
    where
        P: BuildHList,
        R: AllSatisfied<P, W>,
        Mods: ModuleList<<P as BuildHList>::Output, MW>,
    {
        self.try_build_state::<W, MW>()
            .await
            .unwrap_or_else(|e| panic!("Failed to resolve bean dependency graph: {e}"))
    }

    /// Resolve the bean dependency graph and build the HList application
    /// state, returning an error instead of panicking on resolution failure.
    ///
    /// See [`build_state`](Self::build_state).
    pub async fn try_build_state<W, MW>(
        mut self,
    ) -> Result<AppBuilder<<P as BuildHList>::Output>, crate::beans::BeanError>
    where
        P: BuildHList,
        R: AllSatisfied<P, W>,
        Mods: ModuleList<<P as BuildHList>::Output, MW>,
    {
        #[cfg(feature = "dev-reload")]
        {
            type Cached<P> = (
                <P as BuildHList>::Output,
                Arc<crate::beans::BeanContext>,
            );

            let registry = std::mem::take(&mut self.shared.bean_registry);

            // Phase 1: compute graph fingerprint (cheap — no bean construction)
            let (new_fp, per_bean_fps) = registry.compute_fingerprint()?;
            let cached_fp = crate::dev::get_cached_graph_fingerprint();

            // If fingerprint matches and we have a cached state → reuse it
            if let (Some(old_fp), Some((cached_state, cached_ctx))) =
                (cached_fp, crate::dev::get_cached_state::<Cached<P>>())
            {
                if old_fp == new_fp {
                    tracing::debug!(
                        "dev-reload: graph fingerprint unchanged, reusing cached state"
                    );
                    return Ok(Mods::register_controllers(AppBuilder::from_pre(
                        self.shared,
                        cached_state,
                        cached_ctx,
                    )));
                }

                // Fingerprint changed — log only the beans that actually changed
                tracing::info!(
                    "dev-reload: graph fingerprint changed ({:#018x} → {:#018x}), rebuilding all beans",
                    old_fp,
                    new_fp
                );
                let old_per_bean = crate::dev::get_cached_per_bean_fingerprints();
                for (type_id, name, bean_fp) in &per_bean_fps {
                    let changed = old_per_bean
                        .get(type_id)
                        .map(|&old| old != *bean_fp)
                        .unwrap_or(true); // new bean = changed
                    if changed {
                        tracing::info!(
                            bean = name,
                            "dev-reload: bean changed — triggering rebuild"
                        );
                    }
                }

                crate::dev::clear_state_cache();
            }

            // Phase 2: full resolution (construct all beans)
            let ctx = registry.resolve().await?;
            let state = <P as BuildHList>::build_hlist(&ctx);
            let ctx = Arc::new(ctx);

            crate::dev::cache_state(&(state.clone(), Arc::clone(&ctx)));
            crate::dev::cache_graph_fingerprint(new_fp, per_bean_fps);

            Ok(Mods::register_controllers(AppBuilder::from_pre(
                self.shared,
                state,
                ctx,
            )))
        }

        #[cfg(not(feature = "dev-reload"))]
        {
            let registry = std::mem::take(&mut self.shared.bean_registry);
            let ctx = registry.resolve().await?;
            let state = <P as BuildHList>::build_hlist(&ctx);

            Ok(Mods::register_controllers(AppBuilder::from_pre(
                self.shared,
                state,
                Arc::new(ctx),
            )))
        }
    }

}

// `with_state` bypasses the bean graph entirely, so it cannot support pending
// feature modules (their controllers construct from the resolved context) —
// it is only available while `Mods = TNil`.
impl<P, R> AppBuilder<NoState, P, R, TNil> {
    /// Provide a pre-built state directly (backward-compatible path).
    ///
    /// This skips the bean graph entirely. The bean registry is discarded and
    /// the retained bean context is empty. No compile-time provision checking
    /// is performed.
    ///
    /// **Controllers are not supported on this path**: controller cores are
    /// constructed from the bean context, which is empty here — registering a
    /// controller with `#[inject]` fields panics at startup. Use it only for
    /// plugin/raw-router apps (`register_routes`, `merge_router`).
    pub fn with_state<S: Clone + Send + Sync + 'static>(self, state: S) -> AppBuilder<S> {
        AppBuilder::<S>::from_pre(
            self.shared,
            state,
            Arc::new(crate::beans::BeanContext::empty()),
        )
    }
}
