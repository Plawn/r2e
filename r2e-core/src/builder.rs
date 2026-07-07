use crate::beans::{AsyncBean, Bean, BeanRegistry, BeanState, Producer, Registrable};
use crate::controller::Controller;
use crate::lifecycle::{ShutdownHook, StartupHook};
use crate::meta::MetaRegistry;
use crate::service::ServiceComponent;
use crate::plugin::{DeferredAction, DeferredContext, Plugin, RawPreStatePlugin};
use crate::type_list::{AllSatisfied, TAppend, TCons, TNil};
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::info;

type ConsumerReg<T> =
    Box<dyn FnOnce(T) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send>;

type LayerFn = Box<dyn FnOnce(crate::http::Router) -> crate::http::Router + Send>;

/// A meta consumer that drains typed metadata from the registry and returns
/// a router fragment to be merged into the application.
type MetaConsumer<T> = Box<dyn FnOnce(&MetaRegistry) -> crate::http::Router<T> + Send>;

/// A serve hook that receives the shared task registry and a cancellation
/// token. Each hook is responsible for draining the registry of tasks it
/// owns (via `TaskRegistryHandle::take_of::<Tag>()` for tagged tasks, or
/// `take_all()` for single-consumer subsystems).
type ServeHook = Box<dyn FnOnce(TaskRegistryHandle, CancellationToken) + Send>;

/// Shared collection of JobHandles for services spawned via
/// [`AppBuilder::spawn_service`], so shutdown can await their completion
/// with a grace deadline before returning.
#[derive(Clone, Default)]
struct ServiceHandles(Arc<Mutex<Vec<crate::rt::JobHandle<()>>>>);

impl ServiceHandles {
    fn push(&self, handle: crate::rt::JobHandle<()>) {
        self.0.lock().unwrap().push(handle);
    }

    fn drain(&self) -> Vec<crate::rt::JobHandle<()>> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

/// Resolve the active profile: `R2E_PROFILE` env > `r2e.profile` config > `"default"`.
fn resolve_profile(config: &crate::config::R2eConfig) -> String {
    std::env::var("R2E_PROFILE")
        .ok()
        .or_else(|| config.try_get::<String>("r2e.profile"))
        .unwrap_or_else(|| "default".to_string())
}

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
    /// Plugin data storage (type-erased, keyed by TypeId).
    plugin_data: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    /// Name of the last plugin that should be installed last (for ordering validation).
    last_plugin_name: Option<&'static str>,
    /// Whether to install a trailing-slash normalization fallback.
    normalize_path: bool,
    /// Whether the DevReload plugin has been applied (prevents double-install).
    dev_reload_applied: bool,
    /// Maximum time allowed for shutdown hooks to complete before force-exiting.
    /// `None` means wait indefinitely (default).
    shutdown_grace_period: Option<Duration>,
    /// Active profile name, resolved from `R2E_PROFILE` env var, `r2e.profile`
    /// config key, or `"default"`.
    active_profile: String,
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
    state: T,
    routes: Vec<crate::http::Router<T>>,
    startup_hooks: Vec<StartupHook<T>>,
    shutdown_hooks: Vec<ShutdownHook<T>>,
    meta_registry: MetaRegistry,
    meta_consumers: Vec<MetaConsumer<T>>,
    consumer_registrations: Vec<ConsumerReg<T>>,
    /// Serve hooks from plugins (called when server starts).
    /// Tasks already capture their state, so only the token is needed.
    serve_hooks: Vec<ServeHook>,
    /// Shutdown hooks from plugins (sync).
    plugin_shutdown_hooks: Vec<Box<dyn FnOnce() + Send>>,
    /// Shutdown hooks from plugins (async, awaited during shutdown).
    plugin_async_shutdown_hooks: Vec<crate::plugin::AsyncShutdownHook>,
    _provided: PhantomData<P>,
    _required: PhantomData<R>,
}

// ── Conditional assembly (any phase) ────────────────────────────────────────

impl<T: Clone + Send + Sync + 'static, P, R> AppBuilder<T, P, R> {
    /// Conditionally apply a builder transformation.
    ///
    /// `f` must return the **same** builder type, so it may call `Self -> Self`
    /// methods (custom layers, plugins, config toggles) but **not** type-changing
    /// methods like `register`: a runtime flag cannot change the compile-time
    /// provision list `P`. For conditional *bean* presence, use a
    /// `#[producer] -> Option<T>` — the slot is always in `P` and the producer
    /// decides `Some`/`None` internally.
    ///
    /// ```ignore
    /// AppBuilder::new()
    ///     .when(cfg!(debug_assertions), |b| b.with(DevReload))
    /// ```
    pub fn when(self, cond: bool, f: impl FnOnce(Self) -> Self) -> Self {
        if cond {
            f(self)
        } else {
            self
        }
    }
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
                plugin_data: HashMap::new(),
                last_plugin_name: None,
                normalize_path: false,
                dev_reload_applied: false,
                shutdown_grace_period: None,
                active_profile: "default".to_string(),
            },
            state: NoState,
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
        }
    }
}

impl<P, R> AppBuilder<NoState, P, R> {
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
    #[doc(hidden)]
    pub fn with_updated_types<NewP, NewR>(self) -> AppBuilder<NoState, NewP, NewR> {
        AppBuilder {
            shared: self.shared,
            state: NoState,
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
    pub fn register<T: Registrable>(mut self) -> AppBuilder<NoState, TCons<T::Provided, P>, <R as TAppend<T::Deps>>::Output>
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
    pub fn with_default_bean<B: Bean>(mut self) -> AppBuilder<NoState, TCons<B, P>, <R as TAppend<B::Deps>>::Output>
    where
        R: TAppend<B::Deps>,
    {
        self.shared.bean_registry.register_default::<B>();
        self.with_updated_types()
    }

    /// Register a default async bean that can be overridden by alternatives.
    ///
    /// The bean IS added to the provision list (guaranteed to be present).
    pub fn with_default_async_bean<B: AsyncBean>(mut self) -> AppBuilder<NoState, TCons<B, P>, <R as TAppend<B::Deps>>::Output>
    where
        R: TAppend<B::Deps>,
    {
        self.shared.bean_registry.register_async_default::<B>();
        self.with_updated_types()
    }

    /// Register a default producer that can be overridden by alternatives.
    ///
    /// The producer's output IS added to the provision list (guaranteed to be present).
    pub fn with_default_producer<Pr: Producer>(mut self) -> AppBuilder<NoState, TCons<Pr::Output, P>, <R as TAppend<Pr::Deps>>::Output>
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
    ///     .build_state::<Services, _>().await
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
    ///     .build_state::<Services, _>()
    ///     .await
    /// ```
    pub fn with_config(
        mut self,
        config: crate::config::R2eConfig,
    ) -> AppBuilder<NoState, TCons<crate::config::R2eConfig, P>, R> {
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
    ) -> AppBuilder<NoState, TCons<C, TCons<crate::config::R2eConfig, <C::Children as TAppend<P>>::Output>>, R>
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
    ///     .build_state::<Services, _>()
    ///     .await
    /// ```
    pub fn plugin<Pl: RawPreStatePlugin, RIdx>(
        self,
        plugin: Pl,
    ) -> AppBuilder<NoState, <P as TAppend<Pl::Provisions>>::Output, <R as TAppend<Pl::Required>>::Output>
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
    ) -> AppBuilder<NoState, <P as TAppend<Pl::Provisions>>::Output, <R as TAppend<Pl::Required>>::Output>
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
    pub async fn build_state<S, W>(self) -> AppBuilder<S>
    where
        S: BeanState,
        R: TAppend<S::Requires>,
        <R as TAppend<S::Requires>>::Output: AllSatisfied<P, W>,
    {
        self.try_build_state::<S, W>()
            .await
            .expect("Failed to resolve bean dependency graph")
    }

    /// Resolve the bean dependency graph and build the application state,
    /// returning an error instead of panicking on resolution failure.
    pub async fn try_build_state<S, W>(
        mut self,
    ) -> Result<AppBuilder<S>, crate::beans::BeanError>
    where
        S: BeanState,
        R: TAppend<S::Requires>,
        <R as TAppend<S::Requires>>::Output: AllSatisfied<P, W>,
    {
        #[cfg(feature = "dev-reload")]
        {
            let registry = std::mem::replace(&mut self.shared.bean_registry, BeanRegistry::new());

            // Phase 1: compute graph fingerprint (cheap — no bean construction)
            let (new_fp, per_bean_fps) = registry.compute_fingerprint()?;
            let cached_fp = crate::dev::get_cached_graph_fingerprint();

            // If fingerprint matches and we have a cached state → reuse it
            if let (Some(old_fp), Some(cached_state)) =
                (cached_fp, crate::dev::get_cached_state::<S>())
            {
                if old_fp == new_fp {
                    tracing::debug!(
                        "dev-reload: graph fingerprint unchanged, reusing cached state"
                    );
                    return Ok(AppBuilder::<S>::from_pre(self.shared, cached_state));
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

                // Clear the stale monolithic state cache (but keep lifecycle
                // hooks initialized — we only need to rebuild beans, not
                // re-register consumers or re-run startup hooks).
                crate::dev::clear_state_cache();
            }

            // Phase 2: full resolution (construct all beans)
            let ctx = registry.resolve().await?;
            let state = S::from_context(&ctx);

            crate::dev::cache_state(&state);
            crate::dev::cache_graph_fingerprint(new_fp, per_bean_fps);

            return Ok(AppBuilder::<S>::from_pre(self.shared, state));
        }

        #[cfg(not(feature = "dev-reload"))]
        {
            let registry = std::mem::replace(&mut self.shared.bean_registry, BeanRegistry::new());
            let ctx = registry.resolve().await?;
            let state = S::from_context(&ctx);

            Ok(AppBuilder::<S>::from_pre(self.shared, state))
        }
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

/// Resolve the bean graph and build the application state — zero-underscore
/// façade over [`AppBuilder::build_state`].
///
/// [`build_state`](AppBuilder::build_state) carries a single inferred witness
/// type parameter (`build_state::<S, _>()`). This macro hides it so call sites
/// read cleanly:
///
/// ```ignore
/// let app = build_state!(builder, Services).await;
/// // expands to: builder.build_state::<Services, _>().await
/// ```
#[macro_export]
macro_rules! build_state {
    ($app:expr, $state:ty $(,)?) => {
        $app.build_state::<$state, _>()
    };
}

/// Non-panicking variant of [`build_state!`] — façade over
/// [`AppBuilder::try_build_state`].
///
/// ```ignore
/// let app = try_build_state!(builder, Services).await?;
/// // expands to: builder.try_build_state::<Services, _>().await
/// ```
#[macro_export]
macro_rules! try_build_state {
    ($app:expr, $state:ty $(,)?) => {
        $app.try_build_state::<$state, _>()
    };
}

// ── Typed phase (state resolved) ────────────────────────────────────────────

impl<T: Clone + Send + Sync + 'static> AppBuilder<T> {
    /// Internal: construct a typed builder from the pre-state shared config.
    fn from_pre(mut shared: BuilderConfig, state: T) -> Self {
        // Take the deferred actions before creating the builder.
        let deferred_actions = std::mem::take(&mut shared.deferred_actions);

        // Drop the bean registry since it's been consumed.
        shared.bean_registry = BeanRegistry::new();

        let mut builder = Self {
            shared,
            state,
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
    /// The service is constructed from the application state via
    /// [`ServiceComponent::from_state`] and started in a Tokio task during
    /// `on_start`. A [`CancellationToken`] is provided and cancelled
    /// automatically during shutdown.
    ///
    /// # Example
    ///
    /// ```ignore
    /// AppBuilder::new()
    ///     .build_state::<Services, _>().await
    ///     .spawn_service::<MetricsExporter>()
    ///     .serve("0.0.0.0:3000").await
    /// ```
    pub fn spawn_service<C: ServiceComponent<T>>(mut self) -> Self {
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

        self = self.on_start(move |state| async move {
            let service = C::from_state(&state);
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

    /// Register a [`Controller`] whose routes will be merged into the application.
    ///
    /// This also collects event consumers and scheduled task definitions
    /// declared on the controller, so that they are started automatically
    /// by `serve()`.
    pub fn register_controller<C: Controller<T>>(mut self) -> Self {
        C::register_meta(&mut self.meta_registry);

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

        // Construct and bind app-scoped controllers only after config
        // validation, so configuration errors retain their aggregated report.
        let state = &self.state;
        let core = Arc::new(C::from_state(state));
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

        self
    }

    /// Register a bean's event subscriptions.
    ///
    /// The bean is extracted from state via `FromRef` and its
    /// [`EventSubscriber::subscribe()`] method is called during server startup.
    ///
    /// # Example
    ///
    /// ```ignore
    /// AppBuilder::new()
    ///     .build_state::<Services, _>().await
    ///     .register_subscriber::<NotificationService>()
    ///     .serve("0.0.0.0:3000").await.unwrap();
    /// ```
    pub fn register_subscriber<S>(mut self) -> Self
    where
        S: crate::EventSubscriber + crate::http::extract::FromRef<T>,
    {
        self.consumer_registrations.push(Box::new(|state| {
            let subscriber = S::from_ref(&state);
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
        self.build_inner().0
    }

    fn build_inner(
        self,
    ) -> (
        crate::http::Router,
        Vec<StartupHook<T>>,
        Vec<ShutdownHook<T>>,
        Vec<ConsumerReg<T>>,
        Vec<ServeHook>,
        Vec<Box<dyn FnOnce() + Send>>,
        Vec<crate::plugin::AsyncShutdownHook>,
        HashMap<TypeId, Box<dyn Any + Send + Sync>>,
        T,
        Option<Duration>,
    ) {
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

        (
            app,
            self.startup_hooks,
            self.shutdown_hooks,
            self.consumer_registrations,
            self.serve_hooks,
            self.plugin_shutdown_hooks,
            self.plugin_async_shutdown_hooks,
            self.shared.plugin_data,
            state,
            self.shared.shutdown_grace_period,
        )
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

        let (
            app,
            startup_hooks,
            shutdown_hooks,
            consumer_regs,
            serve_hooks,
            plugin_shutdown_hooks,
            plugin_async_shutdown_hooks,
            plugin_data,
            state,
            shutdown_grace_period,
        ) = this.build_inner();

        #[cfg(feature = "quic")]
        let app = if let Some((ref addr, _)) = quic_server_config {
            crate::http::quic::apply_alt_svc(app, addr.port(), quic_alt_svc_max_age)
        } else {
            app
        };

        PreparedApp {
            router: app,
            state,
            addr: addr.to_string(),
            startup_hooks,
            shutdown_hooks,
            consumer_registrations: consumer_regs,
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

/// A fully configured R2E app ready to be served.
///
/// Created by [`AppBuilder::prepare()`]. Holds the assembled router, state,
/// lifecycle hooks, and bind address.
///
/// # Hot-reload
///
/// This type enables the Subsecond hot-reload workflow: build the app inside
/// the hot-patched closure with [`AppBuilder::prepare()`], then call
/// [`.run()`](Self::run) to start serving.
pub struct PreparedApp<T: Clone + Send + Sync + 'static> {
    router: crate::http::Router,
    state: T,
    addr: String,
    startup_hooks: Vec<StartupHook<T>>,
    shutdown_hooks: Vec<ShutdownHook<T>>,
    consumer_registrations: Vec<ConsumerReg<T>>,
    serve_hooks: Vec<ServeHook>,
    plugin_shutdown_hooks: Vec<Box<dyn FnOnce() + Send>>,
    plugin_async_shutdown_hooks: Vec<crate::plugin::AsyncShutdownHook>,
    plugin_data: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    shutdown_grace_period: Option<Duration>,
    tcp_nodelay: bool,
    /// Parsed `server.workers` config. `Ok(None)` → single-listener (default).
    /// `Ok(Some(n))` → SO_REUSEPORT sharded serving with `n` workers.
    /// `Err(msg)` → invalid config value, surfaced as an error at `run()` time.
    workers: Result<Option<usize>, String>,
    #[cfg(feature = "quic")]
    quic_server_config: Option<(std::net::SocketAddr, r2e_http::quic::quinn::ServerConfig)>,
}

/// Internal serving strategy chosen by [`PreparedApp::run`].
///
/// The two variants share the entire lifecycle in
/// [`PreparedApp::run_inner`]; only the bind-and-serve middle section differs.
enum ServeStrategy {
    /// Single listener on the caller's runtime (default behavior, unchanged).
    Single(tokio::net::TcpListener),
    /// SO_REUSEPORT sharded serving: `workers` worker threads, each with its
    /// own `current_thread` runtime and listener on the bound address (first
    /// candidate from `addrs` that binds).
    // Under dev-reload the constructing path (`run_sharded`) is compiled out
    // (sharding + hot-reload is unsupported), so the variant is never built.
    #[cfg_attr(feature = "dev-reload", allow(dead_code))]
    Sharded {
        #[allow(dead_code)]
        addrs: Vec<std::net::SocketAddr>,
        #[allow(dead_code)]
        workers: usize,
    },
}

impl<T: Clone + Send + Sync + 'static> PreparedApp<T> {
    /// Access the assembled router for inspection or testing.
    pub fn router(&self) -> &crate::http::Router {
        &self.router
    }

    /// Mutable access to the router (e.g., for adding test-only routes).
    pub fn router_mut(&mut self) -> &mut crate::http::Router {
        &mut self.router
    }

    /// The application state.
    pub fn state(&self) -> &T {
        &self.state
    }

    /// The bind address.
    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// Whether TCP_NODELAY is enabled for accepted connections.
    pub fn tcp_nodelay(&self) -> bool {
        self.tcp_nodelay
    }

    /// The parsed `server.workers` (SO_REUSEPORT sharding) configuration.
    ///
    /// `Ok(None)` → single-listener serving (default). `Ok(Some(n))` → sharded
    /// serving with `n` worker threads. `Err(msg)` → the config value was
    /// invalid (e.g. `0` or an unknown string); this error is returned by
    /// [`run()`](Self::run).
    pub fn workers(&self) -> Result<Option<usize>, &str> {
        self.workers.as_ref().copied().map_err(|s| s.as_str())
    }

    /// Start listening and serving requests.
    ///
    /// Registers event consumers, runs startup hooks, binds the TCP listener,
    /// and serves with graceful shutdown. After shutdown, runs plugin and user
    /// shutdown hooks.
    pub async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        // Resolve the `server.workers` config; an invalid value is a hard error.
        let workers = self.workers.clone()?;

        match workers {
            // Sharded SO_REUSEPORT serving requested.
            Some(n) => {
                // Hot-reload + sharding is unsupported in v1: the dev-reload
                // listener-caching path bypasses sharding entirely.
                #[cfg(feature = "dev-reload")]
                {
                    let _ = n; // sharding ignored under hot-reload
                    tracing::warn!(
                        "server.workers is set but the `dev-reload` feature is active; \
                         SO_REUSEPORT sharding is ignored (unsupported with hot-reload). \
                         Serving with a single listener."
                    );
                    let listener = crate::dev::get_or_bind_listener(&self.addr)?;
                    self.run_inner(ServeStrategy::Single(listener)).await
                }
                #[cfg(not(feature = "dev-reload"))]
                {
                    self.run_sharded(n).await
                }
            }
            // Default: single listener on the caller's runtime — unchanged.
            None => {
                #[cfg(feature = "dev-reload")]
                let listener = crate::dev::get_or_bind_listener(&self.addr)?;
                #[cfg(not(feature = "dev-reload"))]
                let listener = crate::rt::bind_tcp(&self.addr).await?;
                self.run_inner(ServeStrategy::Single(listener)).await
            }
        }
    }

    /// Sharded SO_REUSEPORT serving. Resolves the bind address once, then
    /// delegates to [`run_inner`](Self::run_inner) with the sharded strategy.
    #[cfg(not(feature = "dev-reload"))]
    async fn run_sharded(self, workers: usize) -> Result<(), Box<dyn std::error::Error>> {
        #[cfg(all(
            unix,
            not(any(target_os = "solaris", target_os = "illumos", target_os = "cygwin"))
        ))]
        {
            // Resolve the address once on the main runtime (async DNS — never
            // blocking std DNS on an async thread). All candidates are kept:
            // the sharded path tries each in order, like `bind_tcp` does.
            let addrs = crate::rt::lookup_host(&self.addr).await?;
            self.run_inner(ServeStrategy::Sharded { addrs, workers })
                .await
        }
        #[cfg(not(all(
            unix,
            not(any(target_os = "solaris", target_os = "illumos", target_os = "cygwin"))
        )))]
        {
            let _ = workers;
            Err(crate::sharded::UNSUPPORTED_PLATFORM_MSG.into())
        }
    }

    /// Like [`run()`](Self::run) but with a pre-bound listener.
    ///
    /// This is useful for hot-reload: bind the listener once in setup,
    /// and reuse it across hot-patches so we never fight port conflicts.
    pub async fn run_with_listener(
        self,
        listener: tokio::net::TcpListener,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Sharding is unsupported on the explicit-listener path: the caller
        // owns the (single) listener. If `server.workers` was configured, warn
        // and proceed single-listener.
        if matches!(self.workers, Ok(Some(_))) {
            tracing::warn!(
                "server.workers is set but run_with_listener was called with an \
                 explicit listener; SO_REUSEPORT sharding is ignored. Serving with \
                 the provided single listener."
            );
        }
        self.run_inner(ServeStrategy::Single(listener)).await
    }

    /// Shared serving core for both single-listener and sharded strategies.
    ///
    /// Owns the full lifecycle: consumer registration, serve/startup hooks,
    /// QUIC spawn, shutdown-future composition, the serve call (single or
    /// sharded), QUIC drain, and the shutdown phase. Only the "bind + serve"
    /// middle differs between strategies.
    async fn run_inner(
        #[cfg_attr(not(feature = "quic"), allow(unused_mut))] mut self,
        strategy: ServeStrategy,
    ) -> Result<(), Box<dyn std::error::Error>> {
        #[cfg(feature = "dev-reload")]
        let skip_lifecycle = crate::dev::is_lifecycle_initialized();
        #[cfg(not(feature = "dev-reload"))]
        let skip_lifecycle = false;

        if !skip_lifecycle {
            // Register event consumers
            for reg in self.consumer_registrations {
                reg(self.state.clone()).await;
            }

            // Call serve hooks (e.g., scheduler starts tasks).
            //
            // Each hook receives a clone of the shared `TaskRegistryHandle`
            // (Arc-backed) and drains the tasks it owns. Multiple hooks can
            // share the registry: scheduler calls `take_all()` or
            // `take_of::<ScheduledTaskMarker>()`, other subsystems pick their
            // own tagged subset, and absent subsystems observe no tasks.
            let task_registry = self.plugin_data
                .get(&TypeId::of::<TaskRegistryHandle>())
                .and_then(|d| d.downcast_ref::<TaskRegistryHandle>())
                .cloned()
                .unwrap_or_default();
            for hook in self.serve_hooks {
                hook(task_registry.clone(), CancellationToken::new());
            }

            // Run startup hooks
            for hook in self.startup_hooks {
                hook(self.state.clone())
                    .await
                    .map_err(|e| -> Box<dyn std::error::Error> { e })?;
            }

            #[cfg(feature = "dev-reload")]
            crate::dev::mark_lifecycle_initialized();
        } else {
            tracing::debug!("dev-reload: skipping consumers, serve hooks, and startup hooks");
        }

        // Pull the shared spawn_service JobHandle collector (if any) so we
        // can await tasks after graceful shutdown.
        let service_handles = self
            .plugin_data
            .get(&TypeId::of::<ServiceHandles>())
            .and_then(|b| b.downcast_ref::<ServiceHandles>())
            .cloned()
            .unwrap_or_default();

        // Compose the shutdown future handed to `with_graceful_shutdown`.
        // When the OS signal arrives, fire plugin shutdown hooks (which
        // cancel tokens handed to spawn_service tasks) BEFORE letting the
        // HTTP server start draining. This way background tasks see the
        // cancel signal while in-flight HTTP requests still get to finish.
        let (plugin_shutdown_hooks, plugin_async_shutdown_hooks) = if skip_lifecycle {
            (Vec::new(), Vec::new())
        } else {
            (self.plugin_shutdown_hooks, self.plugin_async_shutdown_hooks)
        };
        let cancel_token = CancellationToken::new();

        // Spawn the QUIC/HTTP3 endpoint (if configured) before the TCP server.
        // In dev-reload mode, the endpoint is cached so the UDP socket
        // survives across hot-patches without port conflicts.
        #[cfg(feature = "quic")]
        let quic_handle = self.quic_server_config.take().map(|(addr, server_config)| {
            let router = self.router.clone();
            let token = cancel_token.clone();

            #[cfg(feature = "dev-reload")]
            let endpoint_result = crate::dev::get_or_bind_quic_endpoint(addr, server_config);
            #[cfg(not(feature = "dev-reload"))]
            let endpoint_result = crate::http::quic::quinn::Endpoint::server(server_config, addr)
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) });

            match endpoint_result {
                Ok(endpoint) => {
                    #[cfg(not(feature = "dev-reload"))]
                    let ep_for_close = endpoint.clone();
                    Some(crate::rt::spawn(async move {
                        if let Err(e) = crate::http::quic::serve_h3_with_endpoint(
                            router,
                            endpoint,
                            token.cancelled(),
                        )
                        .await
                        {
                            tracing::error!(error = %e, "QUIC/HTTP3 server error");
                        }
                        #[cfg(not(feature = "dev-reload"))]
                        {
                            ep_for_close.close(0u32.into(), b"shutdown");
                            ep_for_close.wait_idle().await;
                        }
                    }))
                }
                Err(e) => {
                    tracing::error!(error = %e, "Failed to bind QUIC endpoint");
                    None
                }
            }
        }).flatten();

        let cancel_for_shutdown = cancel_token.clone();
        let shutdown_future = async move {
            crate::rt::shutdown_signal().await;
            for hook in plugin_shutdown_hooks {
                hook();
            }
            for hook in plugin_async_shutdown_hooks {
                hook().await;
            }
            cancel_for_shutdown.cancel();
        };

        // ── Serve (single-listener or sharded) ──────────────────────────────
        // Only this middle section differs between strategies; the lifecycle
        // start above and the shutdown phase below are shared.
        let serve_result: Result<(), Box<dyn std::error::Error>> = match strategy {
            ServeStrategy::Single(listener) => {
                info!(addr = %self.addr, "R2E server listening");
                let svc = self.router
                    .into_make_service_with_connect_info::<std::net::SocketAddr>();
                if self.tcp_nodelay {
                    use crate::http::ListenerExt as _;
                    crate::http::serve(
                        listener.tap_io(|stream| {
                            if let Err(e) = stream.set_nodelay(true) {
                                tracing::warn!(error = %e, "failed to set TCP_NODELAY on accepted connection");
                            }
                        }),
                        svc,
                    )
                    .with_graceful_shutdown(shutdown_future)
                    .await
                    .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })
                } else {
                    crate::http::serve(listener, svc)
                        .with_graceful_shutdown(shutdown_future)
                        .await
                        .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })
                }
            }
            #[cfg(all(
                unix,
                not(any(target_os = "solaris", target_os = "illumos", target_os = "cygwin"))
            ))]
            ServeStrategy::Sharded { addrs, workers } => {
                // Drive the shutdown future on the main runtime: it awaits the
                // OS signal, fires plugin shutdown hooks, then cancels the
                // shared token. Each worker observes a child token's
                // cancellation as its graceful-shutdown signal.
                let shutdown_handle = crate::rt::spawn(shutdown_future);

                let router = self.router.clone();
                let tcp_nodelay = self.tcp_nodelay;
                let cancel_for_workers = cancel_token.clone();
                // Capture the main (multi-thread) runtime handle as the control
                // plane. Worker threads register it so that background work
                // initiated from request handlers (and lazy-bean first-touch)
                // runs here, not on the workers' current_thread runtimes.
                let control_plane = crate::rt::current_handle();
                if control_plane.runtime_flavor()
                    != tokio::runtime::RuntimeFlavor::MultiThread
                {
                    // A current_thread control plane mostly works, but a
                    // worker-side lazy first-touch would block the worker on a
                    // runtime that may itself be busy — sharding is designed
                    // for a multi-thread main runtime.
                    tracing::warn!(
                        "server.workers is set but run() is driven by a \
                         non-multi-thread runtime; the control plane should be \
                         a multi-thread runtime (use #[tokio::main])"
                    );
                }
                // `serve_sharded` blocks the calling thread joining the worker
                // threads, so run it on a blocking task to avoid stalling the
                // main runtime (which must keep driving the shutdown future).
                let join = crate::rt::spawn_blocking(move || {
                    crate::sharded::serve_sharded(
                        router,
                        &addrs,
                        workers,
                        tcp_nodelay,
                        control_plane,
                        cancel_for_workers,
                    )
                })
                .await;

                // Ensure the shutdown future's task is wound down (it has
                // already fired by the time workers exited, since workers only
                // exit on cancellation).
                shutdown_handle.abort();

                match join {
                    Ok(res) => res.map_err(|e| -> Box<dyn std::error::Error> { e }),
                    Err(e) => Err(format!("sharded serve task failed: {e}").into()),
                }
            }
            #[cfg(not(all(
                unix,
                not(any(target_os = "solaris", target_os = "illumos", target_os = "cygwin"))
            )))]
            ServeStrategy::Sharded { .. } => {
                Err(crate::sharded::UNSUPPORTED_PLATFORM_MSG.into())
            }
        };
        serve_result?;

        // Wait for QUIC endpoint to drain after TCP server stops.
        #[cfg(feature = "quic")]
        if let Some(handle) = quic_handle {
            if let Err(join_err) = handle.await {
                if join_err.is_panic() {
                    tracing::warn!("QUIC task panicked");
                }
            }
        }

        // After HTTP drain completes: await spawn_service JobHandles with a
        // deadline, then run user shutdown hooks. Both phases together are
        // bounded by `shutdown_grace_period` if set.
        let state_for_shutdown = self.state.clone();
        let shutdown_hooks = self.shutdown_hooks;
        let shutdown_phase = async move {
            let handles = service_handles.drain();
            if !handles.is_empty() {
                tracing::info!(
                    count = handles.len(),
                    "Awaiting spawn_service tasks to finish"
                );
                for h in handles {
                    if let Err(e) = h.await {
                        if e.is_panic() {
                            tracing::warn!(error = %e, "spawn_service task panicked");
                        } else if !e.is_cancelled() {
                            tracing::warn!(error = %e, "spawn_service task join error");
                        }
                    }
                }
            }

            for hook in shutdown_hooks {
                hook(state_for_shutdown.clone()).await;
            }
        };

        if let Some(grace) = self.shutdown_grace_period {
            if crate::rt::timeout(grace, shutdown_phase).await.is_err() {
                tracing::warn!(
                    grace_secs = grace.as_secs(),
                    "Shutdown grace period elapsed; some background tasks did not finish in time"
                );
            }
        } else {
            shutdown_phase.await;
        }

        info!("R2E server stopped");
        Ok(())
    }
}

/// Handle to a task registry for collecting background tasks from
/// multiple subsystems (scheduler, gRPC, custom plugins, …).
///
/// Tasks are tagged at insertion time with an owner `TypeId` so each
/// subsystem's serve hook can drain only the tasks it owns. Cloneable
/// (internally `Arc`) so all hooks share the same backing store.
#[derive(Clone)]
pub struct TaskRegistryHandle {
    inner: Arc<Mutex<Vec<TaggedTask>>>,
}

struct TaggedTask {
    owner: TypeId,
    task: Box<dyn Any + Send>,
}

impl TaskRegistryHandle {
    /// Create a new empty task registry handle.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Add type-erased tasks to the registry, tagged with the given owner
    /// marker type. The same marker must be used by the consuming serve
    /// hook's `take_of::<Tag>()` call.
    pub fn add_boxed_for<Tag: 'static>(&self, tasks: Vec<Box<dyn Any + Send>>) {
        let owner = TypeId::of::<Tag>();
        let mut guard = self.inner.lock().unwrap();
        guard.extend(tasks.into_iter().map(|task| TaggedTask { owner, task }));
    }

    /// Add type-erased tasks tagged as "anonymous" (retrievable only by
    /// `take_all`). Retained for the single-consumer case where no tag
    /// marker is available; new call sites should use `add_boxed_for`.
    pub fn add_boxed(&self, tasks: Vec<Box<dyn Any + Send>>) {
        self.add_boxed_for::<AnonymousTask>(tasks);
    }

    /// Drain all tasks tagged with the given owner marker.
    pub fn take_of<Tag: 'static>(&self) -> Vec<Box<dyn Any + Send>> {
        let wanted = TypeId::of::<Tag>();
        let mut guard = self.inner.lock().unwrap();
        let mut kept = Vec::with_capacity(guard.len());
        let mut taken = Vec::new();
        for entry in std::mem::take(&mut *guard) {
            if entry.owner == wanted {
                taken.push(entry.task);
            } else {
                kept.push(entry);
            }
        }
        *guard = kept;
        taken
    }

    /// Drain every task in the registry, regardless of owner tag.
    ///
    /// Intended for single-consumer subsystems (historically the scheduler).
    /// When multiple subsystems register serve hooks, prefer `take_of::<Tag>()`
    /// so each hook only sees its own tasks.
    pub fn take_all(&self) -> Vec<Box<dyn Any + Send>> {
        let mut guard = self.inner.lock().unwrap();
        std::mem::take(&mut *guard)
            .into_iter()
            .map(|e| e.task)
            .collect()
    }
}

/// Marker type used for tasks added via the untagged `add_boxed` path.
struct AnonymousTask;

/// Marker tag for scheduled tasks (interval / cron / delayed) produced by
/// controllers and consumed by the `r2e-scheduler` serve hook.
///
/// Defined in `r2e-core` so both sides — the controller registration path
/// (which doesn't depend on `r2e-scheduler`) and the scheduler's `on_serve`
/// hook — can agree on the tag without introducing a reverse dependency.
pub struct ScheduledTaskMarker;

impl Default for TaskRegistryHandle {
    fn default() -> Self {
        Self::new()
    }
}

// ── Subsecond hot-reload ─────────────────────────────────────────────────
