#[allow(unused_imports)] // referenced by intra-doc links
use super::{Plugin, PluginBuildContext};
use super::{RoutesContext, RoutesEffect};
use std::any::Any;

// ── Deferred action system ─────────────────────────────────────────────────

/// A deferred action that runs after state resolution.
///
/// This is the low-level mechanism for plugins that need to run setup code
/// after `build_state()` is called. Each action is a closure that receives a
/// `DeferredContext` providing access to builder internals.
///
/// Most plugins never construct one directly: the effect sugar on
/// [`PluginBuildContext`] ([`add_layer`](PluginBuildContext::add_layer),
/// [`on_shutdown`](PluginBuildContext::on_shutdown), …) covers the common
/// cases from inside [`build`](Plugin::build). Reach for
/// `PluginSetupContext::add_deferred(DeferredAction::new(..))` only as a
/// pre-graph escape hatch.
///
/// # Example (preferred — build-time sugar)
///
/// ```ignore
/// impl Plugin for MyPlugin {
///     type Provided = (MyToken,);
///     type Deps = ();
///     type Config = ();
///
///     async fn build(
///         self,
///         _deps: (),
///         _config: Option<()>,
///         ctx: &mut PluginBuildContext,
///     ) -> Result<(MyToken,), PluginBuildError> {
///         let token = MyToken::new();
///         let handle = MyHandle::new(token.clone());
///
///         ctx.add_layer(move |router| router.layer(Extension(handle)));
///         ctx.on_shutdown(|| { /* cleanup */ });
///         Ok((token,))
///     }
/// }
/// ```
pub struct DeferredAction {
    /// Name of the action (for debugging/logging).
    pub name: &'static str,
    /// The action to execute.
    pub action: Box<dyn FnOnce(&mut DeferredContext) + Send>,
}

impl DeferredAction {
    /// Create a new deferred action.
    pub fn new<F>(name: &'static str, action: F) -> Self
    where
        F: FnOnce(&mut DeferredContext) + Send + 'static,
    {
        Self {
            name,
            action: Box::new(action),
        }
    }
}

/// A boxed async shutdown hook.
pub type AsyncShutdownHook =
    Box<dyn FnOnce() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send>;

/// Context for executing a deferred action.
///
/// Provides access to builder internals that deferred actions may need to modify.
pub struct DeferredContext<'a> {
    /// Layers to apply to the router.
    #[doc(hidden)]
    pub layers: &'a mut Vec<Box<dyn FnOnce(crate::http::Router) -> crate::http::Router + Send>>,
    /// Transport-level router transforms, applied outermost (after layers and
    /// the catch-panic layer). See [`DeferredContext::wrap_router`].
    #[doc(hidden)]
    pub router_wraps:
        &'a mut Vec<Box<dyn FnOnce(crate::http::Router) -> crate::http::Router + Send>>,
    /// Plugin data storage.
    #[doc(hidden)]
    pub plugin_data:
        &'a mut std::collections::HashMap<std::any::TypeId, Box<dyn Any + Send + Sync>>,
    /// Serve hooks (called when server starts). Each hook receives a
    /// [`ServeContext`](crate::builder::ServeContext) tying it into the
    /// app's shutdown sequence.
    #[doc(hidden)]
    pub serve_hooks: &'a mut Vec<crate::builder::ServeHook>,
    /// Shutdown hooks from plugins (sync).
    #[doc(hidden)]
    pub shutdown_hooks: &'a mut Vec<Box<dyn FnOnce() + Send>>,
    /// Shutdown hooks from plugins (async, awaited during shutdown).
    #[doc(hidden)]
    pub async_shutdown_hooks: &'a mut Vec<AsyncShutdownHook>,
    /// The fully resolved bean graph, available because deferred actions run
    /// after `build_state()`. Read beans out of it via
    /// [`bean_context`](DeferredContext::bean_context).
    #[doc(hidden)]
    pub bean_context: &'a std::sync::Arc<crate::beans::BeanContext>,
    /// The loaded [`R2eConfig`](crate::config::R2eConfig), if any. Deferred
    /// actions run inside `build_state()`, which always follows `load_config` /
    /// `with_config`. `None` only when neither `load_config` nor `with_config`
    /// was called.
    #[doc(hidden)]
    pub config: Option<&'a crate::config::R2eConfig>,
    /// Routes-stage effects, queued here and drained in `build()` once every
    /// controller has registered. See [`DeferredContext::after_routes`].
    #[doc(hidden)]
    pub routes_effects: &'a mut Vec<RoutesEffect>,
    /// Whether to install the pre-routing trailing-slash normalization rewrite.
    /// See [`DeferredContext::enable_normalize_path`].
    #[doc(hidden)]
    pub normalize_path: &'a mut bool,
    /// Whether the dev-reload endpoints/layer have already been installed.
    /// See [`DeferredContext::mark_dev_reload_applied`].
    #[doc(hidden)]
    pub dev_reload_applied: &'a mut bool,
}

impl DeferredContext<'_> {
    /// The fully resolved bean graph.
    ///
    /// Deferred actions run after `build_state()`, so every bean —
    /// `.provide()`-d, `.register()`-ed (factory-built), or produced by another
    /// plugin — is materialized and readable here (`ctx.bean_context().get::<T>()`).
    pub fn bean_context(&self) -> &crate::beans::BeanContext {
        self.bean_context
    }

    /// A **retainable** handle on the resolved bean graph.
    ///
    /// [`bean_context`](Self::bean_context) is a borrow, and
    /// `BeanContext::clone()` deliberately keeps only the shared base (it drops
    /// the overlay of factory-built beans). A deferred action that must hand
    /// the graph to something outliving it takes this handle instead, which
    /// keeps the *whole* graph alive. (Plugins should prefer
    /// [`PluginBuildContext::graph`].)
    pub fn bean_context_handle(&self) -> std::sync::Arc<crate::beans::BeanContext> {
        std::sync::Arc::clone(self.bean_context)
    }

    /// The loaded [`R2eConfig`](crate::config::R2eConfig), if any.
    ///
    /// `Some` whenever `load_config` / `with_config` was called (the reliable
    /// point for config — it always precedes `build_state()`). This is the
    /// low-level counterpart to a plugin's typed [`Config`](crate::Plugin::Config).
    pub fn config(&self) -> Option<&crate::config::R2eConfig> {
        self.config
    }

    /// Add a layer to the router.
    pub fn add_layer(
        &mut self,
        layer: Box<dyn FnOnce(crate::http::Router) -> crate::http::Router + Send>,
    ) {
        self.layers.push(layer);
    }

    /// Add a transport-level router transform, applied **outermost** — after
    /// every [`add_layer`](Self::add_layer) layer (regardless of plugin
    /// install order) and after the built-in catch-panic layer.
    ///
    /// Use this instead of `add_layer` when the transform routes traffic
    /// *around* the HTTP stack (e.g. a content-type multiplexer handing
    /// gRPC requests to tonic): the wrapped-in service sees raw requests
    /// before any HTTP middleware, while the inner HTTP router keeps its
    /// full middleware stack. Do NOT use it for ordinary HTTP middleware —
    /// it would also intercept the non-HTTP branch of any multiplexer
    /// installed by another plugin.
    pub fn wrap_router(
        &mut self,
        wrap: Box<dyn FnOnce(crate::http::Router) -> crate::http::Router + Send>,
    ) {
        self.router_wraps.push(wrap);
    }

    /// Store plugin-specific data for later retrieval.
    ///
    /// Plugins can store arbitrary data keyed by type. This data persists
    /// through the builder lifecycle and can be retrieved during controller
    /// registration or serve hooks.
    pub fn store_data<D: Any + Send + Sync + 'static>(&mut self, data: D) {
        self.plugin_data
            .insert(std::any::TypeId::of::<D>(), Box::new(data));
    }

    /// Remove and return plugin data stored earlier, if present.
    ///
    /// The counterpart of [`store_data`](Self::store_data) /
    /// [`PluginBuildContext::store_data`]: a plugin can stash a non-`Clone`
    /// value and move it out later in an
    /// [`after_build`](crate::plugin::PluginBuildContext::after_build) closure
    /// or another deferred action — e.g. a command channel receiver that must
    /// travel into a serve hook. Returns `None` when no value of type `D` was
    /// stored.
    pub fn take_data<D: Any + Send + Sync + 'static>(&mut self) -> Option<D> {
        self.plugin_data
            .remove(&std::any::TypeId::of::<D>())
            .and_then(|d| d.downcast::<D>().ok())
            .map(|b| *b)
    }

    /// Queue a **Routes-stage** effect: a closure run in `build()`, once every
    /// controller has registered. See
    /// [`PluginBuildContext::after_routes`], the sugar most plugins use.
    pub fn after_routes<F>(&mut self, f: F)
    where
        F: FnOnce(&mut RoutesContext) + Send + 'static,
    {
        self.routes_effects.push(Box::new(f));
    }

    /// Install the pre-routing trailing-slash normalization rewrite
    /// (`/users/` → `/users`), applied before routing so `MatchedPath` reaches
    /// every layer. Idempotent.
    pub fn enable_normalize_path(&mut self) {
        *self.normalize_path = true;
    }

    /// Claim the one-shot dev-reload install slot.
    ///
    /// Returns `true` the first time it is called (the caller should install
    /// the dev endpoints and the `Cache-Control: no-store` layer), `false`
    /// afterwards — so an explicit `.plugin(DevReload)` and `prepare()`'s
    /// automatic install cannot double-mount the routes.
    pub fn mark_dev_reload_applied(&mut self) -> bool {
        if *self.dev_reload_applied {
            return false;
        }
        *self.dev_reload_applied = true;
        true
    }

    /// Add a serve hook that runs when the server starts.
    ///
    /// The hook receives a [`ServeContext`](crate::builder::ServeContext):
    /// the shared task registry (drain the tasks the hook owns via
    /// `take_of::<Tag>()`, or `take_all()` for single-consumer subsystems),
    /// the app shutdown token, and a `track()` collector for spawned tasks
    /// whose drain must be awaited at shutdown.
    pub fn on_serve<F>(&mut self, hook: F)
    where
        F: FnOnce(crate::builder::ServeContext) + Send + 'static,
    {
        self.serve_hooks.push(crate::builder::ServeHook {
            hook: Box::new(hook),
            each_cycle: false,
        });
    }

    /// Like [`on_serve`](Self::on_serve), but **not skipped by `r2e dev`
    /// hot-patch cycles**.
    ///
    /// A hot patch rebuilds the app and drops the previous `run()` future,
    /// which cancels that cycle's shutdown token — tasks the previous serve
    /// hooks tracked observe it and stop (they are detached, not aborted,
    /// so a task that ignores the token keeps running). The startup lifecycle (consumers,
    /// `#[on_start]`, plain `on_serve` hooks, startup hooks) is then skipped
    /// on purpose: it must run once per process, not once per patch. A
    /// transport that owns its own port is the exception: its server task is
    /// gone with the old cycle and nothing would ever serve the rebuilt
    /// routes. Register such a hook here instead; it runs on every `run()`,
    /// and in production exactly once, like `on_serve`.
    ///
    /// The hook must therefore be safe to run again: bind through
    /// [`ServeContext::bind_tcp`](crate::builder::ServeContext::bind_tcp)
    /// (the port carries over between cycles), serve through
    /// [`BoundListener::into_incoming`](crate::builder::BoundListener::into_incoming)
    /// (stops accepting on shutdown *or* the next cycle taking the socket
    /// over, and releases it to that cycle), and never start anything a
    /// second run would duplicate.
    pub fn on_serve_each_cycle<F>(&mut self, hook: F)
    where
        F: FnOnce(crate::builder::ServeContext) + Send + 'static,
    {
        self.serve_hooks.push(crate::builder::ServeHook {
            hook: Box::new(hook),
            each_cycle: true,
        });
    }

    /// Add a shutdown hook that runs when the server stops.
    pub fn on_shutdown<F>(&mut self, hook: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.shutdown_hooks.push(Box::new(hook));
    }

    /// Add an async shutdown hook that is awaited during server shutdown.
    ///
    /// Unlike [`on_shutdown`](Self::on_shutdown), the returned future is awaited
    /// as part of the shutdown sequence, so operations like graceful drain can
    /// actually complete within their configured timeout.
    pub fn on_shutdown_async<F, Fut>(&mut self, hook: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        self.async_shutdown_hooks
            .push(Box::new(move || Box::pin(hook())));
    }
}
