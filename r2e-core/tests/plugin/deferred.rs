//! Raw `DeferredContext` / `DeferredAction` mechanics, plus the two-orders
//! contract: builds execute in topological order, effects apply in install
//! order.

use std::any::Any;
use std::collections::HashMap;

use r2e_core::builder::ServeContext;
use r2e_core::plugin::{
    AsyncShutdownHook, DeferredAction, DeferredContext, PluginBuildContext, PluginBuildError,
    PreStatePlugin,
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

impl PreStatePlugin for ConsumerPlugin {
    type Provided = (Beta,);
    type Deps = (Alpha,);
    type Config = ();

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

impl PreStatePlugin for ProducerPlugin {
    type Provided = (Alpha,);
    type Deps = ();
    type Config = ();

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
