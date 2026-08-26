//! Raw `DeferredContext` / `DeferredAction` mechanics, plus the two-orders
//! contract: builds execute in topological order, effects apply in install
//! order.

use std::any::Any;
use std::collections::HashMap;

use r2e_core::builder::ServeContext;
use r2e_core::plugin::{
    AsyncShutdownHook, DeferredAction, DeferredContext, Plugin, PluginBuildContext,
    PluginBuildError,
};
use r2e_core::AppBuilder;

use crate::fixtures::{Alpha, Beta, EventLog};

// ── Two orders: topo builds, install-order effects ──────────────────────────

/// Installed FIRST but depends on `ProducerPlugin`'s bean — so its build runs
/// SECOND (topological order), while its effects still apply FIRST (install
/// order).
struct ConsumerPlugin {
    log: EventLog,
}

impl Plugin for ConsumerPlugin {
    type Provided = (Beta,);
    type Deps = (Alpha,);
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        (_alpha,): (Alpha,),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(Beta,), PluginBuildError> {
        self.log.push("build-consumer");
        let log = self.log.clone();
        ctx.after_build(move |_dctx| log.push("effect-consumer"));
        Ok((Beta("consumer".into()),))
    }
}

/// Installed SECOND, provides the bean the first plugin depends on.
struct ProducerPlugin {
    log: EventLog,
}

impl Plugin for ProducerPlugin {
    type Provided = (Alpha,);
    type Deps = ();
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(Alpha,), PluginBuildError> {
        self.log.push("build-producer");
        let log = self.log.clone();
        ctx.after_build(move |_dctx| log.push("effect-producer"));
        Ok((Alpha(1),))
    }
}

#[r2e_core::test]
async fn builds_run_in_topo_order_effects_apply_in_install_order() {
    let log = EventLog::default();
    let _app = AppBuilder::new()
        .plugin(ConsumerPlugin { log: log.clone() })
        .plugin(ProducerPlugin { log: log.clone() })
        .build_state()
        .await;

    assert_eq!(
        log.entries(),
        vec![
            // Builds: producer before consumer (topological order — consumer
            // depends on producer's Alpha despite being installed first).
            "build-producer",
            "build-consumer",
            // Effects: consumer before producer (install order).
            "effect-consumer",
            "effect-producer",
        ]
    );
}

// ── Raw DeferredContext mechanics ────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn make_deferred_context<'a>(
    layers: &'a mut Vec<Box<dyn FnOnce(r2e_core::http::Router) -> r2e_core::http::Router + Send>>,
    router_wraps: &'a mut Vec<
        Box<dyn FnOnce(r2e_core::http::Router) -> r2e_core::http::Router + Send>,
    >,
    plugin_data: &'a mut HashMap<std::any::TypeId, Box<dyn Any + Send + Sync>>,
    serve_hooks: &'a mut Vec<Box<dyn FnOnce(ServeContext) + Send>>,
    shutdown_hooks: &'a mut Vec<Box<dyn FnOnce() + Send>>,
    async_shutdown_hooks: &'a mut Vec<AsyncShutdownHook>,
    bean_context: &'a std::sync::Arc<r2e_core::BeanContext>,
) -> DeferredContext<'a> {
    DeferredContext {
        layers,
        router_wraps,
        plugin_data,
        serve_hooks,
        shutdown_hooks,
        async_shutdown_hooks,
        bean_context,
        config: None,
        // Slots the raw-mechanics tests below don't inspect. Leaked so the
        // helper can hand out `&'a mut` without changing every call site.
        routes_effects: Box::leak(Box::new(Vec::new())),
        normalize_path: Box::leak(Box::new(false)),
        dev_reload_applied: Box::leak(Box::new(false)),
    }
}

#[test]
fn deferred_action_stores_name() {
    let action = DeferredAction::new("test-action", |_ctx| {});
    assert_eq!(action.name, "test-action");
}

#[test]
fn deferred_context_add_layer() {
    let mut layers = Vec::new();
    let mut router_wraps = Vec::new();
    let mut plugin_data = HashMap::new();
    let mut serve_hooks = Vec::new();
    let mut shutdown_hooks = Vec::new();
    let mut async_shutdown_hooks = Vec::new();
    let bean_context = std::sync::Arc::new(r2e_core::BeanContext::empty());
    let mut ctx = make_deferred_context(
        &mut layers,
        &mut router_wraps,
        &mut plugin_data,
        &mut serve_hooks,
        &mut shutdown_hooks,
        &mut async_shutdown_hooks,
        &bean_context,
    );
    ctx.add_layer(Box::new(|router| router));
    assert_eq!(layers.len(), 1);
}

#[test]
fn deferred_context_wrap_router_is_separate_from_layers() {
    let mut layers = Vec::new();
    let mut router_wraps = Vec::new();
    let mut plugin_data = HashMap::new();
    let mut serve_hooks = Vec::new();
    let mut shutdown_hooks = Vec::new();
    let mut async_shutdown_hooks = Vec::new();
    let bean_context = std::sync::Arc::new(r2e_core::BeanContext::empty());
    let mut ctx = make_deferred_context(
        &mut layers,
        &mut router_wraps,
        &mut plugin_data,
        &mut serve_hooks,
        &mut shutdown_hooks,
        &mut async_shutdown_hooks,
        &bean_context,
    );
    ctx.wrap_router(Box::new(|router| router));
    assert_eq!(router_wraps.len(), 1);
    assert!(layers.is_empty());
}

#[test]
fn deferred_context_store_data() {
    let mut layers = Vec::new();
    let mut router_wraps = Vec::new();
    let mut plugin_data = HashMap::new();
    let mut serve_hooks = Vec::new();
    let mut shutdown_hooks = Vec::new();
    let mut async_shutdown_hooks = Vec::new();
    let bean_context = std::sync::Arc::new(r2e_core::BeanContext::empty());
    let mut ctx = make_deferred_context(
        &mut layers,
        &mut router_wraps,
        &mut plugin_data,
        &mut serve_hooks,
        &mut shutdown_hooks,
        &mut async_shutdown_hooks,
        &bean_context,
    );
    ctx.store_data(42u32);
    assert!(plugin_data.contains_key(&std::any::TypeId::of::<u32>()));
    let val = plugin_data
        .get(&std::any::TypeId::of::<u32>())
        .unwrap()
        .downcast_ref::<u32>()
        .unwrap();
    assert_eq!(*val, 42);
}

#[test]
fn deferred_context_on_serve() {
    let mut layers = Vec::new();
    let mut router_wraps = Vec::new();
    let mut plugin_data = HashMap::new();
    let mut serve_hooks = Vec::new();
    let mut shutdown_hooks = Vec::new();
    let mut async_shutdown_hooks = Vec::new();
    let bean_context = std::sync::Arc::new(r2e_core::BeanContext::empty());
    let mut ctx = make_deferred_context(
        &mut layers,
        &mut router_wraps,
        &mut plugin_data,
        &mut serve_hooks,
        &mut shutdown_hooks,
        &mut async_shutdown_hooks,
        &bean_context,
    );
    ctx.on_serve(|_serve_ctx| {});
    assert_eq!(serve_hooks.len(), 1);
}

#[test]
fn deferred_context_on_shutdown() {
    let mut layers = Vec::new();
    let mut router_wraps = Vec::new();
    let mut plugin_data = HashMap::new();
    let mut serve_hooks = Vec::new();
    let mut shutdown_hooks = Vec::new();
    let mut async_shutdown_hooks = Vec::new();
    let bean_context = std::sync::Arc::new(r2e_core::BeanContext::empty());
    let mut ctx = make_deferred_context(
        &mut layers,
        &mut router_wraps,
        &mut plugin_data,
        &mut serve_hooks,
        &mut shutdown_hooks,
        &mut async_shutdown_hooks,
        &bean_context,
    );
    ctx.on_shutdown(|| {});
    assert_eq!(shutdown_hooks.len(), 1);
}

// ── The `with_state` graph-bypass path ──────────────────────────────────────

/// A plugin with both slots filled: a setup datum (a plain deferred action)
/// and a build that provides a bean plus a route.
struct BypassedPlugin;

impl Plugin for BypassedPlugin {
    type Provided = (Alpha,);
    type Deps = ();
    type Config = ();
    type Controllers = ();

    fn setup(&mut self, ctx: &mut r2e_core::plugin::PluginSetupContext) {
        ctx.store_data(crate::fixtures::SetupData(3));
    }

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(Alpha,), PluginBuildError> {
        ctx.store_data(crate::fixtures::StoredData(9));
        ctx.add_layer(|router| {
            router.route(
                "/bypassed",
                r2e_core::http::routing::get(|| async { "never-mounted" }),
            )
        });
        Ok((Alpha(1),))
    }
}

#[r2e_core::test]
async fn with_state_bypasses_plugin_builds_without_panicking() {
    // `with_state` throws the bean registry away and runs the deferred actions
    // against an empty graph, so the plugin's group node never runs and its
    // effects slot stays empty. That is a documented no-op — NOT an assertion
    // failure (a `debug_assert!(false)` here panicked every debug build that
    // combined `.plugin()` with `.with_state()`).
    let app = AppBuilder::new().plugin(BypassedPlugin).with_state(());

    // The setup datum is a plain deferred action: it runs on this path.
    assert_eq!(
        app.get_plugin_data::<crate::fixtures::SetupData>()
            .map(|d| d.0),
        Some(3),
        "setup actions still run — they are not graph nodes"
    );
    // Everything `build` would have produced is absent: build never ran.
    assert!(
        app.get_plugin_data::<crate::fixtures::StoredData>()
            .is_none(),
        "build effects must not apply when build never ran"
    );
    let (status, _) = crate::support::send_get(app.build(), "/bypassed").await;
    assert_eq!(
        status,
        r2e_core::http::StatusCode::NOT_FOUND,
        "the plugin's route never materializes on the with_state path"
    );
}
