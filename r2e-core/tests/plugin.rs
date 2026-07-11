use std::any::Any;
use std::collections::HashMap;

use r2e_core::builder::ServeContext;
use r2e_core::plugin::{AsyncShutdownHook, DeferredAction, DeferredContext};

#[allow(clippy::too_many_arguments)]
fn make_deferred_context<'a>(
    layers: &'a mut Vec<Box<dyn FnOnce(r2e_core::http::Router) -> r2e_core::http::Router + Send>>,
    router_wraps: &'a mut Vec<Box<dyn FnOnce(r2e_core::http::Router) -> r2e_core::http::Router + Send>>,
    plugin_data: &'a mut HashMap<std::any::TypeId, Box<dyn Any + Send + Sync>>,
    serve_hooks: &'a mut Vec<Box<dyn FnOnce(ServeContext) + Send>>,
    shutdown_hooks: &'a mut Vec<Box<dyn FnOnce() + Send>>,
    async_shutdown_hooks: &'a mut Vec<AsyncShutdownHook>,
) -> DeferredContext<'a> {
    DeferredContext {
        layers,
        router_wraps,
        plugin_data,
        serve_hooks,
        shutdown_hooks,
        async_shutdown_hooks,
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
    let mut ctx = make_deferred_context(
        &mut layers,
        &mut router_wraps,
        &mut plugin_data,
        &mut serve_hooks,
        &mut shutdown_hooks,
        &mut async_shutdown_hooks,
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
    let mut ctx = make_deferred_context(
        &mut layers,
        &mut router_wraps,
        &mut plugin_data,
        &mut serve_hooks,
        &mut shutdown_hooks,
        &mut async_shutdown_hooks,
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
    let mut ctx = make_deferred_context(
        &mut layers,
        &mut router_wraps,
        &mut plugin_data,
        &mut serve_hooks,
        &mut shutdown_hooks,
        &mut async_shutdown_hooks,
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
    let mut ctx = make_deferred_context(
        &mut layers,
        &mut router_wraps,
        &mut plugin_data,
        &mut serve_hooks,
        &mut shutdown_hooks,
        &mut async_shutdown_hooks,
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
    let mut ctx = make_deferred_context(
        &mut layers,
        &mut router_wraps,
        &mut plugin_data,
        &mut serve_hooks,
        &mut shutdown_hooks,
        &mut async_shutdown_hooks,
    );
    ctx.on_shutdown(|| {});
    assert_eq!(shutdown_hooks.len(), 1);
}
