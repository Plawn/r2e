#[allow(unused_imports)] // referenced by intra-doc links
use super::{plugin_action_name, PreStatePlugin};
use super::{DeferredAction, DeferredContext, GraphHandle};
use std::any::Any;

/// Context passed to [`PreStatePlugin::setup`] — the **rare** pre-graph
/// escape hatch that runs at `.plugin()` time, before the bean graph exists.
///
/// Most plugins never touch it: the plugin's real work happens in
/// [`PreStatePlugin::build`], which runs inside `build_state()` with resolved
/// dependencies and loaded config. Reach for `setup` only for things other
/// **pre-state** code must observe before the graph is built — buffering an
/// early effect, or registering a [`PreDestroy`](crate::PreDestroy) disposer
/// via [`run_pre_destroy`](Self::run_pre_destroy).
///
/// Config is deliberately **not** available here (it may not be loaded yet —
/// `.plugin()` / `load_config` order does not matter). Read config in
/// [`build`](crate::PreStatePlugin::build) instead, where the typed
/// [`Config`](crate::PreStatePlugin::Config) section is guaranteed loaded.
///
/// # No effects here
///
/// This context **cannot** add router layers, wrap the router, or register
/// serve/shutdown hooks. Those are *surface* effects and belong to
/// [`build`](crate::PreStatePlugin::build), the only phase that knows whether
/// the plugin is [`enabled`](PluginBuildContext::enabled): everything a plugin
/// registers here runs unconditionally, so a disabled plugin whose `setup`
/// mounted an admin route would still expose it. What remains is deliberately
/// small — [`store_data`](Self::store_data) for a coordination datum other
/// pre-state code must read (Scheduler's task-registry handle, which
/// `#[scheduled]` collection needs even when `scheduler.enabled = false`),
/// [`run_pre_destroy`](Self::run_pre_destroy), and the raw
/// [`add_deferred`](Self::add_deferred) escape hatch.
pub struct PluginSetupContext {
    deferred: Vec<DeferredAction>,
    /// Buffered [`store_data`](Self::store_data) calls. Flushed as ONE
    /// [`DeferredAction`] by the blanket `RawPreStatePlugin` impl — see
    /// [`flush`](Self::flush).
    sugar: Vec<Box<dyn FnOnce(&mut DeferredContext) + Send>>,
    /// Lifecycle-hook registrars applied to the bean registry at `.plugin()`
    /// time. Backs [`run_pre_destroy`](Self::run_pre_destroy).
    registry_ops: Vec<Box<dyn FnOnce(&mut crate::beans::BeanRegistry) + Send>>,
}

impl PluginSetupContext {
    /// Create a new setup context.
    pub(crate) fn new() -> Self {
        Self {
            deferred: Vec::new(),
            sugar: Vec::new(),
            registry_ops: Vec::new(),
        }
    }

    /// Register a [`PreDestroy`](crate::PreDestroy) disposal hook for one of
    /// this plugin's `Provided` beans, run during graceful shutdown.
    ///
    /// See [`AppBuilder::provide_with_pre_destroy`](crate::AppBuilder::provide_with_pre_destroy)
    /// for the invocation order.
    pub fn run_pre_destroy<B: crate::PreDestroy>(&mut self) {
        self.registry_ops
            .push(Box::new(|reg| reg.register_pre_destroy::<B>()));
    }

    /// Drain the buffered bean-registry lifecycle registrars (internal).
    pub(crate) fn take_registry_ops(
        &mut self,
    ) -> Vec<Box<dyn FnOnce(&mut crate::beans::BeanRegistry) + Send>> {
        std::mem::take(&mut self.registry_ops)
    }

    /// Register a deferred action to run after state resolution — the raw,
    /// **unconditional** pre-graph escape hatch.
    ///
    /// Whatever you do inside the action runs whether or not the plugin ends up
    /// enabled: `setup` runs before config exists, and this hook hands you the
    /// full [`DeferredContext`], which the framework cannot gate for you. Use
    /// it only for work that is correct unconditionally (buffering a datum,
    /// wiring pre-state coordination). Anything that should disappear under
    /// `<prefix>.enabled = false` — routes, layers, serve/shutdown hooks —
    /// belongs in [`build`](crate::PreStatePlugin::build), whose effects are
    /// gated for you.
    ///
    /// # Ordering
    ///
    /// Every action added here runs **before** the buffered
    /// [`store_data`](Self::store_data) action, in the order added.
    ///
    /// Across plugins, deferred work runs **grouped per plugin, in install
    /// order**: `[A.explicit…, A.setup-data, A.build-effects, B.explicit…,
    /// B.setup-data, B.build-effects]`. Note that plugin **build** execution
    /// follows the graph's topological order instead — effects and builds
    /// are ordered independently.
    pub fn add_deferred(&mut self, action: DeferredAction) {
        self.deferred.push(action);
    }

    /// Store a plugin datum other pre-state code must read regardless of
    /// whether the plugin is enabled. Sugar for a
    /// [`DeferredContext::store_data`] call.
    ///
    /// **Unconditional** — like [`add_deferred`](Self::add_deferred), and for
    /// the same reason. This is the ungated *coordination datum*, not an
    /// effect: Scheduler deposits its task-registry handle here so
    /// `#[scheduled]` collection keeps working with `scheduler.enabled =
    /// false`. A datum whose presence should follow the enabled flag belongs in
    /// [`build`](crate::PreStatePlugin::build) via
    /// [`PluginBuildContext::store_data`].
    ///
    /// Buffered; see the ordering note on [`add_deferred`](Self::add_deferred).
    pub fn store_data<D: Any + Send + Sync + 'static>(&mut self, data: D) {
        self.sugar.push(Box::new(move |dctx| dctx.store_data(data)));
    }

    /// Consume the context, returning the deferred actions to install.
    ///
    /// Actions added via [`add_deferred`](Self::add_deferred) come first, in
    /// call order; the buffered [`store_data`](Self::store_data) calls are
    /// appended as a **single** [`DeferredAction`] named `name` (typically the
    /// plugin's short type name, via [`plugin_action_name`]). No buffered data
    /// contributes no action.
    pub(crate) fn flush(self, name: &'static str) -> Vec<DeferredAction> {
        let PluginSetupContext {
            mut deferred,
            sugar,
            ..
        } = self;
        if !sugar.is_empty() {
            deferred.push(DeferredAction::new(name, move |dctx| {
                for op in sugar {
                    op(dctx);
                }
            }));
        }
        deferred
    }
}

/// What one run of [`PreStatePlugin::build`] left for its install-order
/// deferred action: the effects it registered, split by whether the `enabled`
/// gate applies, **and** the `enabled` decision the group factory took from the
/// graph's `R2eConfig`. The flag travels with the effects so the two are never
/// computed twice from two config sources.
pub(crate) struct BuiltEffects {
    pub(crate) enabled: bool,
    /// Surface effects — routes, layers, router wraps, serve hooks, plugin
    /// data, `after_build`. Dropped when the plugin is disabled.
    pub(crate) effects: Vec<Box<dyn FnOnce(&mut DeferredContext) + Send>>,
    /// Cleanup effects (`on_shutdown`/`on_shutdown_async`). Applied
    /// **regardless** of `enabled`: they dispose of what `build` constructed,
    /// and `build` runs even when the plugin is disabled.
    pub(crate) shutdown: Vec<Box<dyn FnOnce(&mut DeferredContext) + Send>>,
}

/// Shared slot between a plugin's group-node factory (which fills it at the end
/// of `build`) and the plugin's install-order deferred action (which takes and
/// applies it — dropping the surface effects when the carried flag says
/// disabled).
///
/// `None` means `build` never ran for this plugin: on the normal path
/// impossible, on the `with_state` graph-bypass path expected (see the take
/// site).
#[derive(Clone, Default)]
pub(crate) struct EffectsSlot(std::sync::Arc<std::sync::Mutex<Option<BuiltEffects>>>);

impl EffectsSlot {
    pub(crate) fn fill(&self, enabled: bool, effects: PluginEffects) {
        *self.0.lock().expect("EffectsSlot poisoned") = Some(BuiltEffects {
            enabled,
            effects: effects.surface,
            shutdown: effects.shutdown,
        });
    }

    pub(crate) fn take(&self) -> Option<BuiltEffects> {
        self.0.lock().expect("EffectsSlot poisoned").take()
    }
}

/// Context passed to [`PreStatePlugin::build`] — effect registration plus the
/// build-time environment (enabled flag, raw config, graph handle).
///
/// Owned by the group node's factory future (no borrows), so `build` can be a
/// real `async fn`. Effects buffered here are applied after graph resolution,
/// at the plugin's install-order slot — and **dropped** when the plugin is
/// disabled via `<prefix>.enabled = false`.
pub struct PluginBuildContext {
    enabled: bool,
    graph: GraphHandle,
    config: Option<crate::config::R2eConfig>,
    effects: PluginEffects,
}

/// The two effect buckets a [`PluginBuildContext`] fills: surface effects
/// (dropped when the plugin is disabled) and cleanup effects (never dropped).
#[derive(Default)]
pub(crate) struct PluginEffects {
    pub(crate) surface: Vec<Box<dyn FnOnce(&mut DeferredContext) + Send>>,
    pub(crate) shutdown: Vec<Box<dyn FnOnce(&mut DeferredContext) + Send>>,
}

impl PluginBuildContext {
    pub(crate) fn new(
        enabled: bool,
        graph: GraphHandle,
        config: Option<crate::config::R2eConfig>,
    ) -> Self {
        Self {
            enabled,
            graph,
            config,
            effects: PluginEffects::default(),
        }
    }

    pub(crate) fn into_effects(self) -> PluginEffects {
        self.effects
    }

    /// Whether the plugin is enabled (`<prefix>.enabled != false`).
    ///
    /// [`build`](PreStatePlugin::build) always runs — check this and return a
    /// cheap, **inert** disabled variant of your beans when `false` (nothing
    /// bound, spawned, or installed globally). Surface effects registered on
    /// this context ([`add_layer`](Self::add_layer),
    /// [`wrap_router`](Self::wrap_router), [`store_data`](Self::store_data),
    /// [`on_serve`](Self::on_serve), [`after_build`](Self::after_build)) are
    /// dropped automatically when disabled; the cleanup hooks
    /// ([`on_shutdown`](Self::on_shutdown),
    /// [`on_shutdown_async`](Self::on_shutdown_async)) are **not** — they
    /// dispose of what this `build` just constructed, so they run either way.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// A deferred-fill, **weak** handle on the final resolved bean graph.
    ///
    /// Empty during `build` (the graph is still being built — take
    /// dependencies through [`Deps`](PreStatePlugin::Deps)); filled right
    /// after `build_state()` resolves successfully. Store it in beans that must
    /// resolve other beans lazily after boot (per-tenant sources, resource
    /// factories) — storing it does **not** keep the graph alive; see
    /// [`GraphHandle`].
    pub fn graph(&self) -> GraphHandle {
        self.graph.clone()
    }

    /// The loaded [`R2eConfig`](crate::config::R2eConfig), if any — the
    /// low-level counterpart to the typed [`Config`](PreStatePlugin::Config)
    /// parameter, for reading keys outside the plugin's own section.
    pub fn config_raw(&self) -> Option<&crate::config::R2eConfig> {
        self.config.as_ref()
    }

    /// Escape hatch: run a closure against the full [`DeferredContext`] after
    /// graph resolution (at the plugin's install-order effect slot). Prefer
    /// the dedicated sugar methods; use this for anything they don't cover
    /// (e.g. `take_data`).
    ///
    /// Dropped (never run) when the plugin is disabled.
    ///
    /// This is also the hook to reach for when an effect must act on the bean
    /// the **graph** exposes rather than the one `build` just made: `dctx`
    /// carries the resolved graph, so resolving here picks up a test's pinned
    /// override, while a captured instance would not (see the partial-pin note
    /// in `docs/claude/plugins.md`).
    pub fn after_build<F>(&mut self, f: F)
    where
        F: FnOnce(&mut DeferredContext) + Send + 'static,
    {
        self.effects.surface.push(Box::new(f));
    }

    // ── Effect sugar (mirrors `DeferredContext`) ────────────────────────────

    /// Add a layer to the router. Applied after graph resolution; dropped when
    /// the plugin is disabled.
    pub fn add_layer<F>(&mut self, layer: F)
    where
        F: FnOnce(crate::http::Router) -> crate::http::Router + Send + 'static,
    {
        self.effects
            .surface
            .push(Box::new(move |dctx| dctx.add_layer(Box::new(layer))));
    }

    /// Add a transport-level router transform applied **outermost**. See
    /// [`DeferredContext::wrap_router`] for when to prefer it over
    /// [`add_layer`](Self::add_layer).
    pub fn wrap_router<F>(&mut self, wrap: F)
    where
        F: FnOnce(crate::http::Router) -> crate::http::Router + Send + 'static,
    {
        self.effects
            .surface
            .push(Box::new(move |dctx| dctx.wrap_router(Box::new(wrap))));
    }

    /// Store plugin-specific data for later retrieval (keyed by type). See
    /// [`DeferredContext::store_data`].
    ///
    /// A surface effect: dropped when the plugin is disabled. For a datum that
    /// must exist regardless, deposit it from `setup`
    /// ([`PluginSetupContext::store_data`]).
    pub fn store_data<D: Any + Send + Sync + 'static>(&mut self, data: D) {
        self.effects
            .surface
            .push(Box::new(move |dctx| dctx.store_data(data)));
    }

    /// Add a serve hook that runs when the server starts. See
    /// [`DeferredContext::on_serve`].
    pub fn on_serve<F>(&mut self, hook: F)
    where
        F: FnOnce(crate::builder::ServeContext) + Send + 'static,
    {
        self.effects
            .surface
            .push(Box::new(move |dctx| dctx.on_serve(hook)));
    }

    /// Add a shutdown hook that runs when the server stops. See
    /// [`DeferredContext::on_shutdown`].
    ///
    /// **Not** gated by [`enabled`](Self::enabled): cleanup follows the
    /// resource `build` constructed, and `build` runs even when the plugin is
    /// disabled. Keep the hook safe to run against a disabled/inert variant.
    ///
    /// # These hooks order shutdown; they do not guarantee it
    ///
    /// Hooks fire in registration order, one at a time, each inside a
    /// `catch_unwind` — a panicking hook cannot discard the ones behind it. But
    /// they only fire on the exits `run()` controls. A panic unwinding out of
    /// the serve loop, or the `run()` future being dropped under an `r2e dev`
    /// hot patch, runs **no** hook at all; there, only cancellation of the app
    /// shutdown token propagates (its drop guard), reaching every token derived
    /// from it — which is how `spawn_service` tasks stop.
    //
    // MUST (declared window, not covered by a test): a plugin that mints a
    // standalone `CancellationToken::new()` and cancels it ONLY from this hook
    // leaves its task running on those two hookless exits. Derive the token
    // from one the framework cancels (the `ServeContext::shutdown_token()`
    // handed to `on_serve`, or relay it like `r2e-scheduler` does) whenever the
    // task must stop on every path.
    pub fn on_shutdown<F>(&mut self, hook: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.effects
            .shutdown
            .push(Box::new(move |dctx| dctx.on_shutdown(hook)));
    }

    /// Add an async shutdown hook awaited during shutdown. See
    /// [`DeferredContext::on_shutdown_async`].
    ///
    /// **Not** gated by [`enabled`](Self::enabled) — see
    /// [`on_shutdown`](Self::on_shutdown).
    pub fn on_shutdown_async<F, Fut>(&mut self, hook: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        self.effects
            .shutdown
            .push(Box::new(move |dctx| dctx.on_shutdown_async(hook)));
    }
}
