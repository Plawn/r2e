use super::{DeferredAction, EffectsSlot, PluginBuildContext, PluginSetupContext};
use crate::builder::{AppBuilder, NoState};
use crate::type_list::{PluginDeps, PluginProvisions, TAppend};

// ── Pre-state Plugin traits ────────────────────────────────────────────────

/// Derive a short, human-readable action name from a plugin type — the last
/// path segment of its type name, before any generic arguments. For example
/// `r2e_prometheus::Prometheus` → `"Prometheus"`.
///
/// Used by the blanket [`RawPreStatePlugin`] impl to name the single
/// [`DeferredAction`] flushed from a plugin's buffered sugar calls, so the
/// plugin author never has to name it themselves.
#[doc(hidden)]
pub fn plugin_action_name<T: ?Sized>() -> &'static str {
    let full = std::any::type_name::<T>();
    let base = full.split('<').next().unwrap_or(full);
    let short = base.rsplit("::").next().unwrap_or(base);
    if short.is_empty() {
        full
    } else {
        short
    }
}

/// A plugin that runs in the pre-state phase and provides beans.
///
/// A pre-state plugin **is one async, fallible factory** for its
/// [`Provided`](Self::Provided) tuple: [`build`](Self::build) runs inside
/// [`build_state()`](crate::AppBuilder::build_state) as a node of the bean
/// graph, topologically ordered after its [`Deps`](Self::Deps), with the
/// typed [`Config`](Self::Config) section guaranteed loaded. Writing a plugin
/// feels like writing a `#[bean]` constructor — no shells, no two-phase
/// install/configure dance.
///
/// ```text
///   .plugin(Me)                 build_state()                (serve)
///        │                           │                          │
///        ▼                           ▼                          ▼
///   [node queued]   ─►  deps built ─► build(deps, config) ─► effects applied
/// ```
///
/// Each element of the `Provided` tuple is projected into the graph as its
/// own bean, so other beans, controllers, and plugins inject them normally —
/// and their construction is a **real** topological edge: a `#[bean]` that
/// injects a plugin-provided type is built after the plugin, and vice versa.
///
/// # Dependencies
///
/// Declare dependencies via [`Deps`](Self::Deps); they arrive **by value** in
/// [`build`](Self::build), already constructed. They are also appended to the
/// builder's requirement list and verified against the **final** provision
/// list at `build_state()` — order-independent, so a dependency may be
/// `.provide()`-d or `.register()`-ed before *or after* the `.plugin()` call:
///
/// ```ignore
/// AppBuilder::new()
///     .plugin(MyPlugin { .. })    // Deps = (DbPool,)
///     .provide(pool)              // ✅ provided after the plugin — fine
///     .build_state().await        // ❌ compile error HERE if DbPool is missing
/// ```
///
/// [`Provided`](Self::Provided) is a **tuple** of beans: `(A,)` for a single
/// bean, `(A, B)` for several, or `()` for none.
///
/// # Effects
///
/// Router layers, serve/shutdown hooks, and plugin data are registered on the
/// [`PluginBuildContext`] during `build` and applied after the graph is
/// resolved. Effects are applied **in plugin install order** (while builds
/// execute in topological order). Surface effects are dropped when the plugin
/// is disabled via `<prefix>.enabled = false`; the cleanup hooks
/// ([`on_shutdown`](PluginBuildContext::on_shutdown) /
/// [`on_shutdown_async`](PluginBuildContext::on_shutdown_async)) survive,
/// because `build` — and whatever it constructed — ran anyway.
///
/// For plugins that need arbitrary builder access (calling `.register()`,
/// `.provide()`, etc. by hand), implement [`RawPreStatePlugin`] instead — but
/// that is rarely necessary. Every `PreStatePlugin` is automatically a
/// [`RawPreStatePlugin`] via a blanket impl, so both work with `.plugin()`.
///
/// # Examples
///
/// Simple plugin (no dependencies):
///
/// ```ignore
/// use r2e_core::{PreStatePlugin, PluginBuildContext, PluginBuildError};
///
/// pub struct MyPlugin { pub value: String }
///
/// impl PreStatePlugin for MyPlugin {
///     type Provided = (String,);
///     type Deps = ();
///     type Config = ();
///
///     async fn build(
///         self,
///         _deps: (),
///         _config: Option<()>,
///         _ctx: &mut PluginBuildContext,
///     ) -> Result<(String,), PluginBuildError> {
///         Ok((self.value,))
///     }
/// }
/// ```
///
/// Plugin whose bean is built from dependencies, with effects:
///
/// ```ignore
/// impl PreStatePlugin for MyPlugin {
///     type Provided = (MyService,);
///     type Deps = (DbPool, CancelToken);
///     type Config = MyConfig;               // #[derive(ConfigProperties)]
///     const CONFIG_PREFIX: Option<&'static str> = Some("my-plugin");
///
///     async fn build(
///         self,
///         (pool, token): (DbPool, CancelToken),
///         config: Option<MyConfig>,
///         ctx: &mut PluginBuildContext,
///     ) -> Result<(MyService,), PluginBuildError> {
///         let service = MyService::connect(pool, config.unwrap_or_default()).await?;
///         let handle = service.handle();
///         ctx.on_shutdown_async(move || async move { handle.drain().await });
///         Ok((service,))
///     }
/// }
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not implement a pre-state plugin trait (`PreStatePlugin`/`RawPreStatePlugin`), the API used by `.plugin()`",
    label = "`.plugin()` needs a pre-state plugin",
    note = "if `{Self}` is a post-state plugin, install it with `.with({Self})` AFTER `build_state()` instead of `.plugin({Self})`"
)]
pub trait PreStatePlugin: Send + Sized + 'static {
    /// The **tuple** of bean types this plugin provides to the bean graph.
    ///
    /// Use `(A,)` for a single bean, `(A, B)` for several, or `()` for none.
    /// Each element must be `Clone + Send + Sync + 'static`; each becomes its
    /// own bean in the graph, projected out of the tuple [`build`](Self::build)
    /// returns.
    type Provided: PluginProvisions;

    /// Bean dependencies this plugin requires, as a concrete tuple —
    /// constructed **before** [`build`](Self::build) (they are real edges in
    /// the bean graph's topological order) and handed to it by value.
    ///
    /// These may name **any** bean — `.provide()`-d values, `.register::<T>()`-ed
    /// (factory-built) beans, and beans other plugins provide. They are
    /// appended to the builder's requirement list and verified against the
    /// **final** provision list at `build_state()` (not at the `.plugin()`
    /// call site), so a dependency may be supplied *after* this plugin is
    /// installed.
    ///
    /// Most plugins set this to `()` (no dependencies). On stable Rust
    /// associated types have no defaults, so every impl must write it
    /// explicitly:
    ///
    /// ```ignore
    /// type Deps = ();
    /// ```
    type Deps: crate::type_list::PluginDeps;

    /// The plugin's typed configuration section.
    ///
    /// Set to `()` (the common case) for a plugin that reads no typed config —
    /// it can still read raw keys via [`PluginBuildContext::config_raw`]. For
    /// typed config, set this to any `#[derive(ConfigProperties)]` struct and
    /// point [`CONFIG_PREFIX`](Self::CONFIG_PREFIX) at its YAML section. The
    /// framework loads and validates that section before calling
    /// [`build`](Self::build) — a malformed value produces the same boot error
    /// a controller's `#[config(section)]` mismatch does.
    ///
    /// On stable Rust associated types have no defaults, so every impl must
    /// write it explicitly (`type Config = ();`).
    type Config: crate::config::PluginConfig;

    /// The YAML section prefix for [`Config`](Self::Config).
    ///
    /// `None` (the default) disables typed-config loading — use it with
    /// `type Config = ();`. `Some("prometheus")` loads the `Config` from the
    /// `prometheus.*` section. The section is treated as **optional**
    /// (presence-based, like a controller's `Option<Section>`): if no config
    /// was loaded, or no key lives under the prefix, [`build`](Self::build)
    /// receives `None`. A present-but-invalid section is a boot error. The
    /// section is parsed whenever it is present — **even when the plugin is
    /// disabled** via `<prefix>.enabled = false` — so it is always structurally
    /// validated; keep semantic (cross-field) validation behind your own
    /// enabled check if it must not fire when disabled.
    const CONFIG_PREFIX: Option<&'static str> = None;

    /// Optional dev-reload stamp mixed into the plugin node's build fingerprint.
    ///
    /// Plugin nodes are volatile — rebuilt on every `r2e dev` hot-patch cycle —
    /// so most plugins never touch this. It exists as a forward-compatible
    /// escape hatch should reuse semantics ever change.
    const BUILD_VERSION: u64 = 0;

    /// Opt in to skipping [`build`](Self::build) entirely when a test pinned
    /// **every** [`Provided`](Self::Provided) type before `.plugin()`.
    ///
    /// Default `false`: `build` always runs, and pinned projections still win
    /// per type (the graph holds the override; the plugin's own value for that
    /// slot is discarded). Leave it alone unless the following is true of your
    /// plugin.
    ///
    /// Set it to `true` **only** when `build` is pure bean construction —
    /// every observable output of the plugin is one of its `Provided` beans,
    /// and it registers no effects (routes, layers, serve/shutdown hooks,
    /// plugin data) — *and* that construction costs something a test wants to
    /// avoid (opening a connection, booting a container, generating keys).
    /// `OpenFga` is the in-tree example: pinning `OpenFgaRegistry`,
    /// `FgaClient`, and `OpenFgaHandle` replaces the whole plugin, so skipping
    /// its gRPC boot is exactly right.
    ///
    /// A plugin that carries **effects** must keep the default. Its routes and
    /// hooks are not part of `Provided`, so "all beans pinned" says nothing
    /// about whether the plugin is still needed: OIDC provides one
    /// `Arc<JwtClaimsValidator>` and registers its `/oauth/token`, discovery,
    /// JWKS, and user-info routes as build effects — under `TestApp`, which
    /// pins the validators, `true` here would silently 404 every OIDC route.
    /// To silence an effect-carrying plugin in a test, disable it
    /// (`<prefix>.enabled = false`) instead.
    const SKIP_BUILD_WHEN_ALL_PINNED: bool = false;

    /// Rare **pre-graph** escape hatch, run once at `.plugin()` time.
    /// Default: no-op.
    ///
    /// Runs before the bean graph exists — and possibly before config is
    /// loaded — so nothing is resolvable here. Use it only for things other
    /// pre-state code must observe (see [`PluginSetupContext`]): lifecycle
    /// registrars, explicit low-level deferred actions. Everything else —
    /// building beans, reading config, registering layers and hooks — belongs
    /// in [`build`](Self::build).
    #[allow(unused_variables)]
    fn setup(&mut self, ctx: &mut PluginSetupContext) {}

    /// Build the plugin's [`Provided`](Self::Provided) beans — **the** plugin.
    ///
    /// Runs inside `build_state()` as a node of the bean graph, topologically
    /// after [`Deps`](Self::Deps) (which arrive fully constructed, by value),
    /// with config guaranteed loaded. May be `async` and may fail: an `Err`
    /// aborts startup with a
    /// [`BeanError::PluginBuild`](crate::beans::BeanError::PluginBuild) naming
    /// the plugin. Side effects (router layers, serve/shutdown hooks, plugin
    /// data) are registered on `ctx` — see [`PluginBuildContext`].
    ///
    /// # `config`
    ///
    /// `Some(cfg)` only when [`CONFIG_PREFIX`](Self::CONFIG_PREFIX) is
    /// `Some(prefix)`, config was loaded, and a key lives under that prefix;
    /// otherwise `None`. Precedence for a config-consuming plugin is: explicit
    /// builder setting (a field on `self`) > `config` (file) > built-in
    /// default.
    ///
    /// # Disabled plugins
    ///
    /// `build` **always runs**, even when `<prefix>.enabled = false` — the
    /// `Provided` types are part of the compile-time provision list, so the
    /// beans must exist. Check [`ctx.enabled()`](PluginBuildContext::enabled)
    /// and return a cheap, inert disabled variant: nothing bound, spawned,
    /// installed globally, or connected. Surface effects registered on `ctx`
    /// are dropped automatically when disabled; the cleanup hooks
    /// (`on_shutdown`/`on_shutdown_async`) are not, so whatever the disabled
    /// path *did* construct is still disposed of.
    ///
    /// # Test pinning
    ///
    /// Pinning a `Provided` type (`override_bean` before `.plugin()`) replaces
    /// that type in the graph — its projection is not registered. `build`
    /// still runs, whatever is pinned: it is the only thing that produces the
    /// plugin's routes, layers and hooks, which are not beans and cannot be
    /// pinned. A plugin whose `build` is *pure* bean construction can opt out
    /// of that with
    /// [`SKIP_BUILD_WHEN_ALL_PINNED`](Self::SKIP_BUILD_WHEN_ALL_PINNED) — then
    /// pinning every `Provided` type skips `build` entirely. To silence an
    /// effect-carrying plugin in a test, disable it via
    /// `<prefix>.enabled = false`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// impl PreStatePlugin for MetricsExporter {
    ///     type Provided = (ExporterHandle,);
    ///     type Deps = (MetricsRegistry,); // registered elsewhere via `.register()`
    ///     type Config = ExporterConfig;
    ///     const CONFIG_PREFIX: Option<&'static str> = Some("exporter");
    ///
    ///     async fn build(
    ///         self,
    ///         (registry,): (MetricsRegistry,),
    ///         config: Option<ExporterConfig>,
    ///         ctx: &mut PluginBuildContext,
    ///     ) -> Result<(ExporterHandle,), PluginBuildError> {
    ///         let handle = ExporterHandle::connect(config.unwrap_or_default()).await?;
    ///         let h = handle.clone();
    ///         ctx.on_serve(move |_sc| h.bind(registry));
    ///         Ok((handle,))
    ///     }
    /// }
    /// ```
    fn build(
        self,
        deps: Self::Deps,
        config: Option<Self::Config>,
        ctx: &mut PluginBuildContext,
    ) -> impl std::future::Future<Output = Result<Self::Provided, PluginBuildError>> + Send;
}

/// The error type [`PreStatePlugin::build`] returns; an `Err` aborts boot with
/// a [`BeanError::PluginBuild`](crate::beans::BeanError::PluginBuild).
pub type PluginBuildError = Box<dyn std::error::Error + Send + Sync>;

/// Internal machinery backing [`PreStatePlugin`] — **not** part of the public
/// plugin-authoring surface.
///
/// This trait is the HList-based, full-builder-access form that `.plugin()`
/// actually dispatches on. Every [`PreStatePlugin`] gets a `RawPreStatePlugin`
/// impl for free via the blanket impl below, which is how multi-bean plugins,
/// deferred actions, and compile-time dependency checking are wired into the
/// builder's type-level provision/requirement lists.
///
/// **Almost no one should implement this directly.** The simplified
/// [`PreStatePlugin`] now supports multiple provided beans (via a tuple
/// [`Provided`](PreStatePlugin::Provided)), so the only remaining reason to
/// hand-write a `RawPreStatePlugin` is to call arbitrary builder methods
/// (`.register()`, `.provide()`, `.when()`, …) during install. It is kept as an
/// escape hatch for that case.
///
/// # `Required = TNil` and `with_updated_types()`
///
/// When `Required` is `TNil`, the compiler cannot prove that
/// `<R as TAppend<TNil>>::Output == R`. Since `R` is a phantom type parameter,
/// call [`.with_updated_types()`](AppBuilder::with_updated_types) at the end of
/// `install()` to perform the zero-cost phantom type conversion.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not implement a pre-state plugin trait (`PreStatePlugin`/`RawPreStatePlugin`), the API used by `.plugin()`",
    label = "`.plugin()` needs a pre-state plugin",
    note = "if `{Self}` is a post-state plugin, install it with `.with({Self})` AFTER `build_state()` instead of `.plugin({Self})`"
)]
pub trait RawPreStatePlugin: Send + 'static {
    /// The type-level list of bean types this plugin provides.
    ///
    /// For a single provision: `TCons<MyType, TNil>`.
    /// For multiple: `TCons<A, TCons<B, TNil>>`.
    type Provisions;

    /// The requirement list appended to the builder's `R`: the plugin's `Deps`.
    ///
    /// Nothing is checked at the `.plugin()` call site — the list rides along
    /// in `R` and is verified against the **final** provision list at
    /// `build_state()`, so a dependency may be provided or registered *after*
    /// this plugin is installed.
    type Required;

    /// Install the plugin in the pre-state phase with full builder access.
    ///
    /// `Mods` is the builder's pending feature-module list — plugins carry it
    /// through unchanged.
    fn install<P, R, Mods>(
        self,
        app: AppBuilder<NoState, P, R, Mods>,
    ) -> crate::builder::WithPluginInstalled<Self, P, R, Mods>
    where
        P: TAppend<Self::Provisions>,
        R: TAppend<Self::Required>;
}

// Blanket impl: every PreStatePlugin is automatically a RawPreStatePlugin.
//
// The plugin's `Provided` tuple maps to the type-level provision list via
// `PluginProvisions::AsList`. At the value level, the plugin becomes bean-graph
// nodes: one **group node** (`PluginOut<T>`, running `build` with resolved deps
// and loaded config) plus one **projection node** per `Provided` element that
// clones its slot out of the group's tuple. The type-level list is then
// advanced in one phantom `with_updated_types()` cast. Projections register
// strict — colliding with an app `.provide()`/`.register()` of the same type
// (or installing the same plugin twice) is a `DuplicateBean` boot error, and a
// `pin_provide` override placed BEFORE `.plugin()` wins per type.
impl<T> RawPreStatePlugin for T
where
    T: PreStatePlugin,
{
    type Provisions = <T::Provided as PluginProvisions>::AsList;
    type Required = <T::Deps as PluginDeps>::AsList;

    fn install<P, R, Mods>(
        self,
        app: AppBuilder<NoState, P, R, Mods>,
    ) -> crate::builder::WithPluginInstalled<Self, P, R, Mods>
    where
        P: TAppend<Self::Provisions>,
        R: TAppend<Self::Required>,
    {
        let name = plugin_action_name::<T>();
        let prefix = T::CONFIG_PREFIX;
        let mut plugin = self;
        let (registry_ops, deferred) = {
            let mut ctx = PluginSetupContext::new();
            plugin.setup(&mut ctx);
            let registry_ops = ctx.take_registry_ops();
            (registry_ops, ctx.flush(name))
        };
        let mut builder = app;
        for action in deferred {
            // Setup actions are UNCONDITIONAL — never gated on
            // `<prefix>.enabled`, and that is sound only because
            // `PluginSetupContext` cannot register surface effects: no
            // `add_layer`, no `wrap_router`, no serve/shutdown hooks. What is
            // left is the pre-graph *coordination datum* (`store_data`) plus the
            // documented raw `add_deferred` escape hatch — things other
            // pre-state code must observe before (and independently of) the
            // plugin doing any work: Scheduler's `TaskRegistryHandle` must exist
            // for `#[scheduled]` collection even when `scheduler.enabled =
            // false`, or the tasks are silently dropped. The enabled gate
            // belongs to build effects, registered on the `PluginBuildContext`.
            builder = builder.add_deferred(action);
        }
        // Apply lifecycle registrars (run_pre_destroy) the plugin opted its
        // `Provided` beans into. NOT gated by `enabled`: the beans still
        // exist, so their lifecycle stays honest.
        for op in registry_ops {
            op(builder.bean_registry_mut());
        }
        // All-pinned skip — OPT-IN (`SKIP_BUILD_WHEN_ALL_PINNED`), because
        // "every provided bean is pinned" is not evidence that the plugin is
        // unnecessary: effects (routes, layers, hooks) are not beans and cannot
        // be pinned. Only a plugin whose whole output is its `Provided` tuple
        // may declare that pinning it all replaces it. An empty `Provided`
        // tuple never takes this path (`all()` on an empty iterator is
        // vacuously true, but a `Provided = ()` plugin exists for its effects).
        let ids = <T::Provided as PluginProvisions>::element_ids();
        let skip_build = T::SKIP_BUILD_WHEN_ALL_PINNED && {
            let registry = builder.bean_registry_mut();
            !ids.is_empty() && ids.iter().all(|(tid, _)| registry.is_pinned(tid))
        };
        if !skip_build {
            // Group node (runs `build`) + per-element projections. Effects
            // registered on the PluginBuildContext land in this shared slot,
            // drained by the deferred action below.
            let effects = EffectsSlot::default();
            builder
                .bean_registry_mut()
                .register_plugin_group(plugin, effects.clone());
            // Effects apply at the plugin's install-order slot (builds execute
            // in topological order — the two orders are independent). This
            // action is also the single place the "disabled" diagnostic is
            // emitted, since exactly one is scheduled per plugin.
            //
            // `enabled` is NOT recomputed here: the slot carries the decision
            // the group factory made from the graph's `R2eConfig`. Re-reading
            // the builder's own config could disagree with it (a pinned
            // `R2eConfig` bean), and then a plugin could build enabled and have
            // its effects dropped, or vice versa. One decision, one owner.
            builder = builder.add_deferred(DeferredAction::new(name, move |dctx| {
                let Some(built) = effects.take() else {
                    // The graph-bypass path: `.plugin(P).with_state(S)` throws
                    // the bean registry away and runs the deferred actions
                    // against an empty graph, so the group node never ran and
                    // there is nothing to apply — not a bug, a documented
                    // no-op (see `AppBuilder::with_state`). On the normal path
                    // the slot is always filled: the group node is non-lazy and
                    // `volatile`, so it is constructed on every resolution path
                    // (including a dev-reload cache hit), and a failing `build`
                    // aborts boot before any deferred action runs.
                    tracing::debug!(
                        plugin = name,
                        "plugin build never ran (`with_state` bypasses the bean graph); \
                         effects skipped",
                    );
                    return;
                };
                // Cleanup is NOT a surface effect: `on_shutdown`/
                // `on_shutdown_async` dispose of what `build` constructed, and
                // `build` runs whether or not the plugin is enabled. Dropping
                // them with the surface would leak exactly the resources a
                // disabled plugin still built (a disabled Executor's pool would
                // never drain). They run first so a disabled plugin's cleanup is
                // registered even though nothing else of it is.
                for op in built.shutdown {
                    op(dctx);
                }
                if !built.enabled {
                    tracing::info!(
                        plugin = name,
                        "plugin disabled via `{}.enabled = false`; effects skipped (its beans remain in the graph, its cleanup hooks still run)",
                        prefix.unwrap_or(name),
                    );
                    return;
                }
                for op in built.effects {
                    op(dctx);
                }
            }));
        }
        builder.with_updated_types()
    }
}

/// Load and validate a plugin's typed [`Config`](PreStatePlugin::Config)
/// section from an already-resolved `R2eConfig` (the graph-provided bean),
/// at plugin-build time.
///
/// The section is optional (presence-based, like a controller's
/// `Option<Section>`): returns `None` when config loading is disabled
/// (`CONFIG_PREFIX == None`), no config was loaded, or no key lives under the
/// prefix. A present-but-invalid section panics with the same validation report
/// a controller `#[config]` mismatch produces (`plugin` names the plugin in the
/// message) — even when the plugin is disabled via `<prefix>.enabled = false`.
pub(crate) fn load_plugin_config_from<T: PreStatePlugin>(
    config: Option<&crate::config::R2eConfig>,
    plugin: &str,
) -> Option<T::Config> {
    use crate::config::PluginConfig;

    let prefix = T::CONFIG_PREFIX?;
    let config = config?;
    if !config.has_prefix(prefix) {
        return None;
    }
    let errors = <T::Config as PluginConfig>::plugin_validate(config, prefix);
    if !errors.is_empty() {
        panic!(
            "Invalid configuration for plugin `{plugin}` (section `{prefix}`):\n{}",
            crate::config::ConfigValidationError { errors }
        );
    }
    Some(
        <T::Config as PluginConfig>::plugin_load(config, prefix)
            .expect("plugin config section validated but failed to construct"),
    )
}

/// The group-node bean for a plugin `Pl`: the whole `Provided` tuple, built by
/// one run of [`PreStatePlugin::build`]. Projection nodes clone individual
/// elements out of it. Internal — never inject this type.
#[doc(hidden)]
pub struct PluginOut<Pl: PreStatePlugin>(pub Pl::Provided);

// Manual impl: `#[derive(Clone)]` would bound `Pl: Clone`, but only the tuple
// needs to be cloneable (and `PluginProvisions: Clone` guarantees it).
impl<Pl: PreStatePlugin> Clone for PluginOut<Pl> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

/// Whether a plugin's post-state effects should run, per the `<prefix>.enabled`
/// convention (phase 6).
///
/// Returns `true` (enabled) when the plugin declares no `CONFIG_PREFIX`, when no
/// config was loaded, or when the `<prefix>.enabled` key is absent — the flag
/// defaults to `true`, so plugins are on unless explicitly turned off. Only an
/// explicit `<prefix>.enabled = false` disables them.
pub(crate) fn plugin_config_enabled(
    config: Option<&crate::config::R2eConfig>,
    prefix: Option<&'static str>,
) -> bool {
    let (Some(prefix), Some(config)) = (prefix, config) else {
        return true;
    };
    config
        .get::<bool>(&format!("{prefix}.enabled"))
        .unwrap_or(true)
}
