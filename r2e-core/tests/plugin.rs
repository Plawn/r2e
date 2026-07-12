use std::any::Any;
use std::collections::HashMap;

use r2e_core::builder::ServeContext;
use r2e_core::plugin::{AsyncShutdownHook, DeferredAction, DeferredContext};
use r2e_core::type_list::BeanAccess;
use r2e_core::{AppBuilder, PluginInstallContext, PreStatePlugin};

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

// ── Tuple `Provided` (PreStatePlugin) ──────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
struct Alpha(u32);

#[derive(Clone, Debug, PartialEq)]
struct Beta(String);

/// Provides a single bean via the one-tuple `(T,)` form.
struct SingleProvider;

impl PreStatePlugin for SingleProvider {
    type Provided = (Alpha,);
    type Deps = ();

    fn install(self, (): (), _ctx: &mut PluginInstallContext<'_>) -> (Alpha,) {
        (Alpha(7),)
    }
}

/// Provides two beans in one plugin — the case that used to require
/// `RawPreStatePlugin`.
struct MultiProvider;

impl PreStatePlugin for MultiProvider {
    type Provided = (Alpha, Beta);
    type Deps = ();

    fn install(self, (): (), _ctx: &mut PluginInstallContext<'_>) -> (Alpha, Beta) {
        (Alpha(42), Beta("hello".into()))
    }
}

/// Provides nothing — only registers a deferred action.
struct NoProvider;

impl PreStatePlugin for NoProvider {
    type Provided = ();
    type Deps = ();

    fn install(self, (): (), ctx: &mut PluginInstallContext<'_>) {
        ctx.add_deferred(DeferredAction::new("no-provider", |_dctx| {}));
    }
}

#[r2e_core::test]
async fn zero_provision_plugin_builds_and_keeps_other_beans() {
    // `type Provided = ()` maps to TNil: nothing is added to the state, and
    // the builder still accepts the plugin (and its deferred action).
    let app = AppBuilder::new()
        .plugin(NoProvider)
        .provide(Alpha(1))
        .build_state()
        .await;
    assert_eq!(app.state().get::<Alpha>(), Alpha(1));
}

#[r2e_core::test]
async fn single_provision_plugin_resolves_from_state() {
    let app = AppBuilder::new().plugin(SingleProvider).build_state().await;
    let state = app.state();
    assert_eq!(state.get::<Alpha>(), Alpha(7));
    // Also resolvable through the retained bean context (the `#[inject]` path).
    assert_eq!(app.bean_context().as_ref().get::<Alpha>(), Alpha(7));
}

#[r2e_core::test]
async fn multi_provision_plugin_resolves_both_beans_from_state() {
    let app = AppBuilder::new().plugin(MultiProvider).build_state().await;
    let state = app.state();
    assert_eq!(state.get::<Alpha>(), Alpha(42));
    assert_eq!(state.get::<Beta>(), Beta("hello".into()));
    // Both are injectable via the bean context, by type.
    assert_eq!(app.bean_context().as_ref().get::<Alpha>(), Alpha(42));
    assert_eq!(app.bean_context().as_ref().get::<Beta>(), Beta("hello".into()));
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
