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
                router_wraps: Vec::new(),
                bean_registry: BeanRegistry::new(),
                deferred_actions: Vec::new(),
                plugin_data: HashMap::new(),
                last_plugin_name: None,
                normalize_path: false,
                dev_reload_applied: false,
                shutdown_grace_period: None,
                active_profile: "default".to_string(),
                forced_profile: None,
                config_file: None,
                config_overrides: Vec::new(),
                preloaded_config: None,
                stop_handle: None,
                bean_disposers: Vec::new(),
            },
            state: NoState,
            bean_context: Arc::new(crate::beans::BeanContext::empty()),
            routes: Vec::new(),
            startup_hooks: Vec::new(),
            shutdown_hooks: Vec::new(),
            drain_hooks: Vec::new(),
            meta_registry: MetaRegistry::new(),
            meta_consumers: Vec::new(),
            consumer_registrations: Vec::new(),
            post_construct_registrations: Vec::new(),
            serve_hooks: Vec::new(),
            plugin_shutdown_hooks: Vec::new(),
            plugin_async_shutdown_hooks: Vec::new(),
            controller_disposers: Vec::new(),
            bean_disposers: Vec::new(),
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

    /// Mutable access to the bean registry (for internal use by the blanket
    /// `PreStatePlugin` impl to deposit a plugin's provided beans).
    pub(crate) fn bean_registry_mut(&mut self) -> &mut BeanRegistry {
        &mut self.shared.bean_registry
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
            drain_hooks: self.drain_hooks,
            meta_registry: self.meta_registry,
            meta_consumers: self.meta_consumers,
            consumer_registrations: self.consumer_registrations,
            post_construct_registrations: self.post_construct_registrations,
            serve_hooks: self.serve_hooks,
            plugin_shutdown_hooks: self.plugin_shutdown_hooks,
            plugin_async_shutdown_hooks: self.plugin_async_shutdown_hooks,
            controller_disposers: self.controller_disposers,
            bean_disposers: self.bean_disposers,
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
    pub fn provide<B: Clone + Send + Sync + 'static>(
        mut self,
        bean: B,
    ) -> AppBuilder<NoState, TCons<B, P>, R, Mods> {
        self.shared.bean_registry.provide(bean);
        self.with_updated_types()
    }

    /// Provide a pre-built bean **and** run its
    /// [`PostConstruct`](crate::PostConstruct) hook once the graph is resolved.
    ///
    /// Like [`provide`](Self::provide), but the value opts into the same
    /// lifecycle as a factory bean's `#[post_construct]`: the hook fires during
    /// [`build_state`](Self::build_state), **after** every factory-bean
    /// post-construct, and a failure surfaces as the same
    /// [`BeanError::PostConstruct`](crate::BeanError::PostConstruct). The hook
    /// reads the bean by type from the resolved graph, so a pinned test override
    /// is the value it runs against.
    pub fn provide_with_post_construct<B: Clone + Send + Sync + 'static + crate::PostConstruct>(
        mut self,
        bean: B,
    ) -> AppBuilder<NoState, TCons<B, P>, R, Mods> {
        self.shared.bean_registry.provide(bean);
        self.shared
            .bean_registry
            .register_provided_post_construct::<B>();
        self.with_updated_types()
    }

    /// Provide a pre-built bean **and** register its
    /// [`PreDestroy`](crate::PreDestroy) disposal hook.
    ///
    /// The hook runs during graceful shutdown, as part of the async
    /// shutdown-hook phase, after plugin shutdown hooks and in reverse
    /// registration order relative to other bean disposers. It reads the bean by
    /// type from the resolved graph (override-aware).
    pub fn provide_with_pre_destroy<B: Clone + Send + Sync + 'static + crate::PreDestroy>(
        mut self,
        bean: B,
    ) -> AppBuilder<NoState, TCons<B, P>, R, Mods> {
        self.shared.bean_registry.provide(bean);
        self.shared.bean_registry.register_pre_destroy::<B>();
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
    /// Requires [`load_config`](Self::load_config) to have been called first.
    /// Combine it
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
            .expect("config_flag requires config — call .load_config() first")
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
    /// The profile is set when [`load_config`](Self::load_config) is called.
    /// Before that, it is `"default"`.
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
    pub fn with_default_producer<Pr: Producer>(
        mut self,
    ) -> Registered<Pr::Output, Pr::Deps, P, R, Mods>
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

    /// Load configuration and provide it to the builder.
    ///
    /// Combines loading, optional typed construction, and registration in one call:
    /// 1. Loads `application.yaml` (or the file set via
    ///    [`with_config_file`](Self::with_config_file)) + `.env` + env var
    ///    overlay — **unless** a config was stashed via
    ///    [`override_config`](Self::override_config), in which case that
    ///    in-memory config replaces the disk read.
    /// 2. If `C` implements [`ConfigProperties`](crate::config::ConfigProperties),
    ///    constructs the typed config and provides it as a bean
    /// 3. Stores the raw config + provides `R2eConfig` in the bean registry
    ///
    /// This is the ONLY registration point for config: it honors the profile,
    /// the `application-{profile}.yaml` overlay, and
    /// [`override_config_value`](Self::override_config_value) (drained *after*
    /// the base config, so it always wins — including over
    /// [`override_config`](Self::override_config)).
    ///
    /// # Panics
    ///
    /// Panics if configuration loading or typed construction fails, or if both
    /// [`override_config`](Self::override_config) and
    /// [`with_config_file`](Self::with_config_file) were set (the file could
    /// not be honored — the pre-loaded config wins).
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
        let profile = self.shared.forced_profile.as_deref();
        let mut config = match self.shared.preloaded_config.take() {
            // A pre-loaded config (override_config / dev-reload) replaces the
            // disk read entirely.
            Some(preloaded) => {
                assert!(
                    self.shared.config_file.is_none(),
                    "override_config() and with_config_file() are mutually exclusive: \
                     the pre-loaded config replaces the disk read, so the custom file \
                     would be silently ignored. Drop one of them."
                );
                preloaded
            }
            // An explicitly requested file must exist (load_profiled_from is
            // strict, and its error names the file); the default is optional.
            None => match self.shared.config_file.take() {
                Some(file) => crate::config::R2eConfig::load_profiled_from(&file, profile),
                None => crate::config::R2eConfig::load_profiled(profile),
            }
            .unwrap_or_else(|e| panic!("Failed to load config: {e}")),
        };
        for (key, value) in self.shared.config_overrides.drain(..) {
            config.set(&key, value);
        }
        C::register(&config, &mut self.shared.bean_registry)
            .unwrap_or_else(|e| panic!("Failed to construct typed config: {e}"));
        self.shared.active_profile =
            resolve_profile(self.shared.forced_profile.as_deref(), &config);
        self.shared.config = Some(config.clone());
        self.shared.bean_registry.provide(config);
        self.with_updated_types()
    }

    // ── Test-harness pre-configuration ─────────────────────────────────

    /// Pin a bean instance that wins over **any later** registration of the
    /// same type: subsequent `provide`/`register` calls for that type are
    /// silently ignored.
    ///
    /// This is the mock/test-double primitive. A test harness applies its
    /// overrides *before* the application's `App::build` runs, and the app's
    /// own registration of the real bean becomes a no-op — the pinned instance
    /// fills the provision slot instead:
    ///
    /// ```ignore
    /// TestApp::boot_with::<MyApp>(|b| b.override_bean(FakeMailer::new())).await
    /// ```
    ///
    /// The pinned value does not extend the compile-time provision list `P`:
    /// presence is still guaranteed by the app's own registration, only the
    /// value is substituted. Pinning a type the app never registers simply
    /// adds an unused instance.
    pub fn override_bean<B: Clone + Send + Sync + 'static>(mut self, value: B) -> Self {
        self.shared.bean_registry.pin_provide(value);
        self
    }

    /// Like [`override_bean`](Self::override_bean), but ALSO decorates the
    /// pinned instance: its interceptor chains are built from the resolved
    /// graph at `build_state()` and its decorator slot is filled.
    ///
    /// Plain `override_bean` skips **all** of the bean's registration hooks, so
    /// a pinned instance runs undecorated (its `#[intercept]` sites never
    /// fire). This sibling re-enables **decoration only**: it queues the
    /// deco-fill hook via
    /// [`register_deco_fill`](crate::beans::BeanRegistry::register_deco_fill)
    /// (TypeId-deduped, run once against the final graph). Everything else the
    /// pin skips stays skipped — the bean's `#[scheduled]` tasks are still NOT
    /// collected and its `#[post_construct]` still does NOT run.
    ///
    /// ```ignore
    /// TestApp::boot_with::<MyApp>(|b| {
    ///     b.override_bean_decorated(CleanupService::new(stub_pool))
    /// }).await
    /// ```
    pub fn override_bean_decorated<B: crate::decorator::BeanDecoFill + Clone>(
        mut self,
        value: B,
    ) -> Self {
        self.shared.bean_registry.pin_provide(value);
        self.shared.bean_registry.register_deco_fill::<B>();
        self
    }

    /// Supply a pre-loaded [`R2eConfig`](crate::config::R2eConfig) that
    /// [`load_config`](Self::load_config) consumes **in place of** its disk
    /// read.
    ///
    /// This only stashes the config — it registers nothing on its own.
    /// `load_config` remains the sole registration point (it honors the
    /// profile, the `application-{profile}.yaml` overlay, and
    /// [`override_config_value`](Self::override_config_value)); this method just
    /// replaces where the base config comes from. So an app that calls
    /// `override_config` **must** still call `load_config` (any variant,
    /// e.g. `.load_config::<()>()`), otherwise the config is silently ignored —
    /// [`build_state`](Self::build_state) panics to catch that mistake.
    ///
    /// It exists for test harnesses that build an in-memory config (the
    /// full-config sibling of [`override_config_value`](Self::override_config_value)).
    ///
    /// [`override_config_value`](Self::override_config_value) still wins over
    /// this config regardless of call order (its overrides are drained after
    /// the base config in `load_config`). Combining it with
    /// [`with_config_file`](Self::with_config_file) panics in `load_config` —
    /// the file could not be honored.
    ///
    /// # Panics
    ///
    /// Panics if config was already loaded (`override_config` after
    /// `load_config` could never be consumed — call it earlier in the chain).
    pub fn override_config(mut self, config: crate::config::R2eConfig) -> Self {
        assert!(
            self.shared.config.is_none(),
            "override_config() was called after load_config() — the pre-loaded \
             config would never be consumed. Move override_config() before \
             load_config() in the builder chain."
        );
        // Resolve the profile eagerly so reads between override_config and
        // load_config (e.g. conditional assembly) see the right value.
        self.shared.active_profile =
            resolve_profile(self.shared.forced_profile.as_deref(), &config);
        self.shared.preloaded_config = Some(config);
        self
    }

    /// Override a single config key on top of whatever
    /// [`load_config`](Self::load_config) loads — regardless of call order, and
    /// winning over [`override_config`](Self::override_config).
    ///
    /// If config is already loaded, the key is set immediately; otherwise it
    /// is stashed and applied right after loading (the `@TestProfile`
    /// config-override equivalent).
    pub fn override_config_value(
        mut self,
        key: impl Into<String>,
        value: impl Into<crate::config::ConfigValue>,
    ) -> Self {
        match self.shared.config.as_mut() {
            Some(config) => {
                config.set(&key.into(), value.into());
                // Keep the registry copy in sync with the patched config.
                self.shared.bean_registry.provide(config.clone());
            }
            None => {
                self.shared
                    .config_overrides
                    .push((key.into(), value.into()));
            }
        }
        self
    }

    /// Use a custom base config file instead of `application.yaml`.
    ///
    /// Applies to a later [`load_config`](Self::load_config). The profile
    /// overlay file is derived from the base name (`patina.yaml` + profile
    /// `test` → `patina-test.yaml`); secret resolution, the env overlay, and
    /// [`override_config_value`](Self::override_config_value) apply as usual.
    /// Combining this with [`override_config`](Self::override_config) (a
    /// pre-loaded config) is a panic — the file could not be honored.
    ///
    /// ```ignore
    /// AppBuilder::new()
    ///     .with_config_file("patina.yaml")
    ///     .load_config::<RootConfig>()
    /// ```
    pub fn with_config_file(mut self, file: impl Into<std::path::PathBuf>) -> Self {
        self.shared.config_file = Some(file.into());
        self
    }

    /// Force the active profile, winning over `R2E_PROFILE` and `r2e.profile`.
    ///
    /// A later [`load_config`](Self::load_config) also overlays
    /// `application-{profile}.yaml`. Test harnesses use
    /// `with_profile("test")` instead of mutating the process environment
    /// (which would race with parallel tests).
    pub fn with_profile(mut self, profile: impl Into<String>) -> Self {
        let profile = profile.into();
        self.shared.active_profile = profile.clone();
        self.shared.forced_profile = Some(profile);
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
    /// use r2e_executor::Executor;
    ///
    /// AppBuilder::new()
    ///     .plugin(Scheduler)  // Provides CancellationToken + ScheduledJobRegistry
    ///     .plugin(Executor)   // Scheduler runs ticks on the shared pool
    ///     .build_state()
    ///     .await
    /// ```
    pub fn plugin<Pl: RawPreStatePlugin, RIdx>(
        self,
        plugin: Pl,
    ) -> WithPluginInstalled<Pl, P, R, Mods>
    where
        P: TAppend<Pl::Provisions>,
        R: TAppend<Pl::AllRequired>,
        // Only the pre-state `Deps` (`Required`) are checked here, against the
        // provisions present so far. The `LateDeps` portion of `AllRequired` is
        // appended to `R` and verified against the final provision list at
        // `build_state()`.
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
        R: TAppend<Pl::AllRequired>,
        Pl::Required: AllSatisfied<P, RIdx>,
    {
        plugin.install(self)
    }

    /// Add a deferred action to be executed after state resolution.
    ///
    /// This is the low-level escape hatch. [`PreStatePlugin`] implementations
    /// usually reach for the sugar methods on [`PluginInstallContext`]
    /// (`ctx.add_layer(..)`, `ctx.on_shutdown(..)`, …) instead — those buffer
    /// plain closures and are flushed into a single deferred action.
    ///
    /// # Example (sugar — preferred)
    ///
    /// ```ignore
    /// impl PreStatePlugin for MyPlugin {
    ///     type Provided = (MyToken,);
    ///     type Deps = ();
    ///
    ///     fn install(self, (): (), ctx: &mut PluginInstallContext<'_>) -> (MyToken,) {
    ///         let token = MyToken::new();
    ///         let handle = MyHandle::new(token.clone());
    ///
    ///         ctx.add_layer(move |router| router.layer(Extension(handle)));
    ///         ctx.on_shutdown(|| { /* cleanup */ });
    ///         (token,)
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
    pub(crate) fn register_module_impl<M, DepIdx, ExpIdx, CtrlIdx, PlugIdx>(
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
        // Every required plugin must already be installed (its provisions in P).
        M::RequiredPlugins: RequiredPluginsInstalled<P, PlugIdx>,
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
        assert!(
            self.shared.preloaded_config.is_none(),
            "override_config() was set but load_config() was never called — the \
             config would be silently ignored; add .load_config::<()>() (or a \
             typed variant) to the app assembly"
        );
        assert!(
            self.shared.config.is_some() || self.shared.config_overrides.is_empty(),
            "override_config_value() was set but load_config() was never called — \
             the override(s) would be silently ignored; add .load_config::<()>() \
             (or a typed variant) to the app assembly"
        );
        assert!(
            self.shared.config_file.is_none(),
            "with_config_file() was set but load_config() was never called — the \
             file would be silently ignored; add .load_config::<()>() (or a typed \
             variant) to the app assembly"
        );

        let mut registry = std::mem::take(&mut self.shared.bean_registry);
        let scheduled_sources = registry.take_scheduled_sources();
        let event_subscribers = registry.take_event_subscribers();

        // Only inside the actual Subsecond hot-patch loop (`r2e::launch!`
        // marks it) do the process-global dev-reload caches engage. Merely
        // compiling with the feature — tests, examples' passthrough — keeps
        // every build cold, so unrelated builds in one process never serve
        // each other's cached graphs.
        #[cfg(feature = "dev-reload")]
        if crate::dev::hot_reload_loop_active() {
            type Cached<P> = (<P as BuildHList>::Output, Arc<crate::beans::BeanContext>);

            // Phase 1: compute graph fingerprint (cheap — no bean construction)
            let (new_fp, per_bean_fps) = registry.compute_fingerprint()?;
            let cached_fp = crate::dev::get_cached_graph_fingerprint();
            let requires_resolution = registry.requires_resolution_on_cache_hit();

            // If the fingerprint matches and the typed state cache holds →
            // reuse the whole state when no per-cycle hook needs a freshly
            // resolved context. Decorator fills and pre-destroy disposers must
            // pass through `resolve_reusing` even when every bean is reusable.
            if cached_fp == Some(new_fp) && !requires_resolution {
                if let Some((cached_state, cached_ctx)) =
                    crate::dev::get_cached_state::<Cached<P>>()
                {
                    tracing::debug!(
                        "dev-reload: graph fingerprint unchanged, reusing cached state"
                    );
                    // Bean scheduled tasks and subscriptions are re-collected
                    // against the cached graph: the task registry and consumer
                    // registrations are fresh per build (plugins re-install),
                    // even when the bean instances are reused.
                    return Ok(Mods::register_controllers(
                        AppBuilder::from_pre(self.shared, cached_state, cached_ctx)
                            .collect_bean_scheduled_tasks(scheduled_sources)
                            .collect_bean_subscribers(event_subscribers),
                    ));
                }
                // Same graph but the provision list changed shape (e.g. a
                // `.provide()` was added, so `Cached<P>` no longer downcasts):
                // fall through to a rebuild that reuses every bean instance.
            }

            // Phase 2: partial rebuild. Beans whose per-bean fingerprint is
            // unchanged since the previous cycle keep their instance (and
            // in-memory state); changed beans and their transitive
            // dependents — whose fingerprints change by propagation — are
            // reconstructed against the fresh config.
            let reuse_plan = match (cached_fp, crate::dev::get_cached_ctx()) {
                (Some(old_fp), Some(old_ctx)) => {
                    let old_per_bean = crate::dev::get_cached_per_bean_fingerprints();
                    if old_fp != new_fp {
                        tracing::info!(
                            "dev-reload: graph fingerprint changed ({:#018x} → {:#018x}), rebuilding changed beans",
                            old_fp,
                            new_fp
                        );
                        for (type_id, name, bean_fp) in &per_bean_fps {
                            let changed = old_per_bean
                                .get(type_id)
                                .map(|&old| old != *bean_fp)
                                .unwrap_or(true); // new bean = changed
                            if changed {
                                tracing::info!(
                                    bean = name,
                                    "dev-reload: bean changed — rebuilding"
                                );
                            }
                        }
                    }
                    let unchanged: std::collections::HashSet<std::any::TypeId> = per_bean_fps
                        .iter()
                        .filter(|(tid, _, fp)| old_per_bean.get(tid) == Some(fp))
                        .map(|(tid, _, _)| *tid)
                        .collect();
                    Some(crate::beans::ReusePlan { old_ctx, unchanged })
                }
                // First cycle (or explicitly invalidated): full resolution.
                _ => None,
            };

            let mut ctx = registry.resolve_reusing(reuse_plan).await?;
            self.shared.bean_disposers = ctx.take_disposers();
            let state = <P as BuildHList>::build_hlist(&ctx);
            let ctx = Arc::new(ctx);

            crate::dev::cache_state(&(state.clone(), Arc::clone(&ctx)));
            crate::dev::cache_ctx(&ctx);
            crate::dev::cache_graph_fingerprint(new_fp, per_bean_fps);

            return Ok(Mods::register_controllers(
                AppBuilder::from_pre(self.shared, state, ctx)
                    .collect_bean_scheduled_tasks(scheduled_sources)
                    .collect_bean_subscribers(event_subscribers),
            ));
        }

        // Cold path: prod, tests, and dev-reload builds outside the loop.
        let mut ctx = registry.resolve().await?;
        self.shared.bean_disposers = ctx.take_disposers();
        let state = <P as BuildHList>::build_hlist(&ctx);

        Ok(Mods::register_controllers(
            AppBuilder::from_pre(self.shared, state, Arc::new(ctx))
                .collect_bean_scheduled_tasks(scheduled_sources)
                .collect_bean_subscribers(event_subscribers),
        ))
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
