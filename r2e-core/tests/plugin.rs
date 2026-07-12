use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use http_body_util::BodyExt;
use r2e_core::builder::ServeContext;
use r2e_core::http::routing::get;
use r2e_core::http::{Body, Request, StatusCode};
use r2e_core::plugin::{
    plugin_action_name, AsyncShutdownHook, DeferredAction, DeferredContext, PluginInstallContext,
    PreStatePlugin,
};
use r2e_core::type_list::BeanAccess;
use r2e_core::AppBuilder;
use tower::ServiceExt;

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

// ── PluginInstallContext sugar (Phase 2) ────────────────────────────────────

#[derive(Clone)]
struct SugarMarker;

/// Data deposited via `ctx.store_data` sugar.
struct StoredData(u32);

async fn get_route(router: r2e_core::http::Router, path: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

/// A plugin that reaches only for the buffered sugar surface — no
/// `DeferredAction` in sight.
struct SugarBuildPlugin;

impl PreStatePlugin for SugarBuildPlugin {
    type Provided = (SugarMarker,);
    type Deps = ();

    fn install(self, (): (), ctx: &mut PluginInstallContext<'_>) -> (SugarMarker,) {
        ctx.store_data(StoredData(42));
        ctx.add_layer(|router| router.route("/sugar", get(|| async { "sugar-ok" })));
        ctx.wrap_router(|router| router.route("/wrapped", get(|| async { "wrapped-ok" })));
        (SugarMarker,)
    }
}

#[r2e_core::test]
async fn sugar_add_layer_store_data_land_and_execute() {
    let app = AppBuilder::new()
        .plugin(SugarBuildPlugin)
        .build_state()
        .await;

    // `store_data` sugar was flushed into plugin_data at build_state.
    assert_eq!(app.get_plugin_data::<StoredData>().map(|d| d.0), Some(42));

    // `add_layer` and `wrap_router` sugar produced reachable routes.
    let router = app.build();
    let (status, body) = get_route(router.clone(), "/sugar").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "sugar-ok");
    let (status, body) = get_route(router, "/wrapped").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "wrapped-ok");
}

#[derive(Clone, Default)]
struct EventLog(Arc<Mutex<Vec<&'static str>>>);

impl EventLog {
    fn push(&self, event: &'static str) {
        self.0.lock().unwrap().push(event);
    }
    fn entries(&self) -> Vec<&'static str> {
        self.0.lock().unwrap().clone()
    }
}

/// Exercises every serve/shutdown sugar method AND an explicit `add_deferred`
/// escape hatch, so the documented ordering rule (explicit actions run before
/// the single buffered sugar action) is observable end-to-end.
struct EveryHookPlugin {
    log: EventLog,
}

impl PreStatePlugin for EveryHookPlugin {
    type Provided = (SugarMarker,);
    type Deps = ();

    fn install(self, (): (), ctx: &mut PluginInstallContext<'_>) -> (SugarMarker,) {
        let log = self.log;

        // Escape hatch: explicit actions run BEFORE the buffered sugar action.
        let l_es = log.clone();
        let l_esh = log.clone();
        ctx.add_deferred(DeferredAction::new("explicit", move |dctx| {
            dctx.on_serve(move |_sc| l_es.push("explicit-serve"));
            dctx.on_shutdown(move || l_esh.push("explicit-shutdown"));
        }));

        // Sugar hooks — plain closures, no boxing.
        let l_ss = log.clone();
        ctx.on_serve(move |_sc| l_ss.push("sugar-serve"));
        let l_ssh = log.clone();
        ctx.on_shutdown(move || l_ssh.push("sugar-shutdown"));
        let l_sa = log.clone();
        ctx.on_shutdown_async(move || async move { l_sa.push("sugar-async-shutdown") });

        (SugarMarker,)
    }
}

#[tokio::test]
async fn sugar_serve_and_shutdown_hooks_execute_after_explicit() {
    let log = EventLog::default();
    let app = AppBuilder::new()
        .plugin(EveryHookPlugin { log: log.clone() })
        .build_state()
        .await;

    let prepared = app.prepare("127.0.0.1:0");
    let stop = prepared.stop_handle();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = tokio::spawn(async move {
        prepared
            .run_with_listener(listener)
            .await
            .map_err(|e| e.to_string())
    });

    // Let the serve hooks run, then stop and await a clean shutdown.
    tokio::time::sleep(Duration::from_millis(100)).await;
    stop.stop();
    let result = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server did not stop within 5s")
        .expect("server task panicked");
    assert!(result.is_ok(), "run() returned an error: {result:?}");

    let entries = log.entries();

    // Serve hooks executed; the explicit action ran before the sugar action.
    let es = entries.iter().position(|e| *e == "explicit-serve");
    let ss = entries.iter().position(|e| *e == "sugar-serve");
    assert!(
        es.is_some() && ss.is_some(),
        "both serve hooks ran: {entries:?}"
    );
    assert!(es < ss, "explicit action runs before sugar action: {entries:?}");

    // Shutdown hooks (sync + async) executed; explicit before sugar.
    let esh = entries.iter().position(|e| *e == "explicit-shutdown");
    let ssh = entries.iter().position(|e| *e == "sugar-shutdown");
    assert!(
        esh.is_some() && ssh.is_some(),
        "both shutdown hooks ran: {entries:?}"
    );
    assert!(
        esh < ssh,
        "explicit shutdown runs before sugar shutdown: {entries:?}"
    );
    assert!(
        entries.contains(&"sugar-async-shutdown"),
        "async shutdown hook ran: {entries:?}"
    );
}

#[test]
fn plugin_action_name_trims_to_last_segment() {
    // A path-qualified type collapses to its final segment…
    assert_eq!(plugin_action_name::<SugarBuildPlugin>(), "SugarBuildPlugin");
    // …and a primitive with no path is returned as-is.
    assert_eq!(plugin_action_name::<u32>(), "u32");
}
