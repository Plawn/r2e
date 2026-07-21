use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use http_body_util::BodyExt;
use r2e_core::beans::{Bean, BeanContext, BeanRegistry, Registrable};
use r2e_core::builder::ServeContext;
use r2e_core::http::routing::get;
use r2e_core::http::{Body, Request, StatusCode};
use r2e_core::plugin::{
    plugin_action_name, AsyncShutdownHook, DeferredAction, DeferredContext, PluginInstallContext,
    PreStatePlugin,
};
use r2e_core::type_list::BeanAccess;
use r2e_core::{AppBuilder, PostConstruct, PreDestroy, TNil};
use std::any::TypeId;
use tower::ServiceExt;

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
    bean_context: &'a r2e_core::BeanContext,
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
    let bean_context = r2e_core::BeanContext::empty();
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
    let bean_context = r2e_core::BeanContext::empty();
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
    let bean_context = r2e_core::BeanContext::empty();
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
    let bean_context = r2e_core::BeanContext::empty();
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
    type Config = ();

    fn install(&mut self, _ctx: &mut PluginInstallContext<'_>) -> (Alpha,) {
        (Alpha(7),)
    }
}

/// Provides two beans in one plugin — the case that used to require
/// `RawPreStatePlugin`.
struct MultiProvider;

impl PreStatePlugin for MultiProvider {
    type Provided = (Alpha, Beta);
    type Deps = ();
    type Config = ();

    fn install(&mut self, _ctx: &mut PluginInstallContext<'_>) -> (Alpha, Beta) {
        (Alpha(42), Beta("hello".into()))
    }
}

/// Provides nothing — only registers a deferred action.
struct NoProvider;

impl PreStatePlugin for NoProvider {
    type Provided = ();
    type Deps = ();
    type Config = ();

    fn install(&mut self, ctx: &mut PluginInstallContext<'_>) {
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
    assert_eq!(
        app.bean_context().as_ref().get::<Beta>(),
        Beta("hello".into())
    );
}

#[test]
fn deferred_context_on_shutdown() {
    let mut layers = Vec::new();
    let mut router_wraps = Vec::new();
    let mut plugin_data = HashMap::new();
    let mut serve_hooks = Vec::new();
    let mut shutdown_hooks = Vec::new();
    let mut async_shutdown_hooks = Vec::new();
    let bean_context = r2e_core::BeanContext::empty();
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
    type Config = ();

    fn install(&mut self, ctx: &mut PluginInstallContext<'_>) -> (SugarMarker,) {
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
    type Config = ();

    fn install(&mut self, ctx: &mut PluginInstallContext<'_>) -> (SugarMarker,) {
        let log = self.log.clone();

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
    assert!(
        es < ss,
        "explicit action runs before sugar action: {entries:?}"
    );

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

// ── Deps + configure ──────────────────────────────────────────

/// A shared sink the `configure` hook writes into, so the test can observe
/// which `Deps` value it received.
#[derive(Clone, Default)]
struct ConfigureLog(Arc<Mutex<Vec<u32>>>);

impl ConfigureLog {
    fn push(&self, value: u32) {
        self.0.lock().unwrap().push(value);
    }
    fn values(&self) -> Vec<u32> {
        self.0.lock().unwrap().clone()
    }
}

/// A **factory-built** bean: constructed by the bean graph, never handed to
/// `.provide()`. Its `build` stamps a recognizable value so a test can tell the
/// factory-built instance apart from a hand-provided one.
#[derive(Clone, Debug, PartialEq)]
struct FactoryBean(u32);

impl Bean for FactoryBean {
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    fn build(_ctx: &BeanContext) -> Self {
        FactoryBean(99)
    }
}

impl Registrable for FactoryBean {
    type Provided = Self;
    type Deps = TNil;
    fn register_into(registry: &mut BeanRegistry) {
        registry.register::<Self>();
    }
}

/// `Deps = (Alpha,)` where `Alpha` is `.provide()`-d after the plugin.
struct LateProvidedPlugin {
    log: ConfigureLog,
}

impl PreStatePlugin for LateProvidedPlugin {
    type Provided = (ConfigureLog,);
    type Deps = (Alpha,);
    type Config = ();

    fn install(&mut self, _ctx: &mut PluginInstallContext<'_>) -> (ConfigureLog,) {
        (self.log.clone(),)
    }

    fn configure(
        self,
        (log,): &(ConfigureLog,),
        (alpha,): (Alpha,),
        _config: Option<()>,
        _ctx: &mut DeferredContext<'_>,
    ) {
        log.push(alpha.0);
    }
}

#[r2e_core::test]
async fn late_deps_resolves_provided_bean_in_configure() {
    let log = ConfigureLog::default();
    // `Alpha` is provided AFTER `.plugin()` — `Deps` is not checked at the
    // call site, only against the final provision list at `build_state()`.
    let _app = AppBuilder::new()
        .plugin(LateProvidedPlugin { log: log.clone() })
        .provide(Alpha(7))
        .build_state()
        .await;
    assert_eq!(
        log.values(),
        vec![7],
        "configure received the provided Alpha"
    );
}

/// THE acceptance test's plugin: `Deps = (FactoryBean,)` — a bean that only
/// the graph can build, registered *after* this plugin.
struct LateFactoryPlugin {
    log: ConfigureLog,
}

impl PreStatePlugin for LateFactoryPlugin {
    type Provided = (ConfigureLog,);
    type Deps = (FactoryBean,);
    type Config = ();

    fn install(&mut self, _ctx: &mut PluginInstallContext<'_>) -> (ConfigureLog,) {
        (self.log.clone(),)
    }

    fn configure(
        self,
        (log,): &(ConfigureLog,),
        (fb,): (FactoryBean,),
        _config: Option<()>,
        _ctx: &mut DeferredContext<'_>,
    ) {
        log.push(fb.0);
    }
}

#[r2e_core::test]
async fn late_deps_resolves_factory_built_bean_registered_after_plugin() {
    let log = ConfigureLog::default();
    // `FactoryBean` is `.register()`-ed AFTER the plugin. Under the old `Deps`
    // machinery this would panic at runtime ("registered but not materialized");
    // as a `Deps` it resolves from the fully built graph in `configure`.
    let app = AppBuilder::new()
        .plugin(LateFactoryPlugin { log: log.clone() })
        .register::<FactoryBean>()
        .build_state()
        .await;
    // configure saw the factory-built instance (build() stamped 99).
    assert_eq!(log.values(), vec![99]);
    // …and the same bean is a normal member of the resolved graph.
    assert_eq!(
        app.bean_context().as_ref().get::<FactoryBean>(),
        FactoryBean(99)
    );
}

/// The "producer" side of the cross-plugin case: provides `Alpha` at install.
struct AlphaProviderPlugin;

impl PreStatePlugin for AlphaProviderPlugin {
    type Provided = (Alpha,);
    type Deps = ();
    type Config = ();

    fn install(&mut self, _ctx: &mut PluginInstallContext<'_>) -> (Alpha,) {
        (Alpha(11),)
    }
}

#[r2e_core::test]
async fn late_deps_resolves_bean_provided_by_another_plugin() {
    // Producer installed first.
    let log = ConfigureLog::default();
    let _app = AppBuilder::new()
        .plugin(AlphaProviderPlugin)
        .plugin(LateProvidedPlugin { log: log.clone() })
        .build_state()
        .await;
    assert_eq!(
        log.values(),
        vec![11],
        "consumer configure saw producer's Alpha"
    );

    // Consumer installed first: `Deps` binds against the final graph, not
    // install order, so the result is identical.
    let log = ConfigureLog::default();
    let _app = AppBuilder::new()
        .plugin(LateProvidedPlugin { log: log.clone() })
        .plugin(AlphaProviderPlugin)
        .build_state()
        .await;
    assert_eq!(log.values(), vec![11], "install order must not matter");
}

/// Records, in `configure`, both the `provided` argument (the plugin's own
/// instance) and the graph's view of the same bean (via `Deps`) — the
/// pin-override contract documented on `PreStatePlugin::configure`.
struct PinContractPlugin {
    log: ConfigureLog,
}

impl PreStatePlugin for PinContractPlugin {
    type Provided = (Alpha, ConfigureLog);
    type Deps = (Alpha,);
    type Config = ();

    fn install(&mut self, _ctx: &mut PluginInstallContext<'_>) -> (Alpha, ConfigureLog) {
        (Alpha(11), self.log.clone())
    }

    fn configure(
        self,
        (own_alpha, log): &(Alpha, ConfigureLog),
        (graph_alpha,): (Alpha,),
        _config: Option<()>,
        _ctx: &mut DeferredContext<'_>,
    ) {
        log.push(own_alpha.0);
        log.push(graph_alpha.0);
    }
}

#[r2e_core::test]
async fn configure_provided_arg_keeps_own_instance_under_pin_override() {
    let log = ConfigureLog::default();
    let app = AppBuilder::new()
        // Pin an override BEFORE the plugin installs, as a test harness would.
        .override_bean(Alpha(99))
        .plugin(PinContractPlugin { log: log.clone() })
        .build_state()
        .await;
    // The state and the graph hold the pinned override…
    assert_eq!(app.state().get::<Alpha>(), Alpha(99));
    // …while configure's `provided` arg keeps the plugin's own instance (11)
    // and its `Deps` view reflects the override (99).
    assert_eq!(log.values(), vec![11, 99]);
}

/// `configure` reaching for the `DeferredContext` surface (store_data + a layer).
struct LateConfigureCtxPlugin;

impl PreStatePlugin for LateConfigureCtxPlugin {
    type Provided = (SugarMarker,);
    type Deps = (Alpha,);
    type Config = ();

    fn install(&mut self, _ctx: &mut PluginInstallContext<'_>) -> (SugarMarker,) {
        (SugarMarker,)
    }

    fn configure(
        self,
        _p: &(SugarMarker,),
        (alpha,): (Alpha,),
        _config: Option<()>,
        ctx: &mut DeferredContext<'_>,
    ) {
        let v = alpha.0;
        ctx.store_data(StoredData(v));
        ctx.add_layer(Box::new(move |router| {
            router.route("/late", get(move || async move { format!("late-{v}") }))
        }));
    }
}

#[r2e_core::test]
async fn configure_can_use_deferred_context() {
    let app = AppBuilder::new()
        .plugin(LateConfigureCtxPlugin)
        .provide(Alpha(5))
        .build_state()
        .await;

    // `store_data` from configure landed in plugin_data.
    assert_eq!(app.get_plugin_data::<StoredData>().map(|d| d.0), Some(5));

    // …and the layer configure added produced a reachable route.
    let router = app.build();
    let (status, body) = get_route(router, "/late").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "late-5");
}

// ── Typed plugin Config (Phase 4) ────────────────────────────────────────────

/// An all-optional config section, so its presence — not any required key —
/// drives whether `configure` gets `Some`.
#[derive(r2e_core::prelude::ConfigProperties, Clone, Debug, Default, PartialEq)]
struct DemoConfig {
    name: Option<String>,
    count: Option<i64>,
}

/// A config section with a **required** field, used to exercise validation.
#[derive(r2e_core::prelude::ConfigProperties, Clone, Debug)]
struct StrictConfig {
    port: i64,
}

/// Records the `Option<Config>` its `configure` receives, so tests can assert on
/// the presence/values the framework delivered.
struct ConfigReadingPlugin {
    sink: Arc<Mutex<Option<Option<DemoConfig>>>>,
}

impl PreStatePlugin for ConfigReadingPlugin {
    type Provided = ();
    type Deps = ();
    type Config = DemoConfig;
    const CONFIG_PREFIX: Option<&'static str> = Some("demo");

    fn install(&mut self, _ctx: &mut PluginInstallContext<'_>) {}

    fn configure(
        self,
        _p: &(),
        (): (),
        config: Option<DemoConfig>,
        _ctx: &mut DeferredContext<'_>,
    ) {
        *self.sink.lock().unwrap() = Some(config);
    }
}

/// A plugin whose `configure` must never run because validation panics first.
struct StrictConfigPlugin;

impl PreStatePlugin for StrictConfigPlugin {
    type Provided = ();
    type Deps = ();
    type Config = StrictConfig;
    const CONFIG_PREFIX: Option<&'static str> = Some("demo");

    fn install(&mut self, _ctx: &mut PluginInstallContext<'_>) {}

    fn configure(
        self,
        _p: &(),
        (): (),
        _config: Option<StrictConfig>,
        _ctx: &mut DeferredContext<'_>,
    ) {
    }
}

#[r2e_core::test]
async fn plugin_config_loaded_from_present_section() {
    let sink = Arc::new(Mutex::new(None));
    let config = r2e_core::R2eConfig::from_yaml_str("demo:\n  name: hello\n  count: 5\n").unwrap();
    let _app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(ConfigReadingPlugin { sink: sink.clone() })
        .build_state()
        .await;

    let received = sink.lock().unwrap().clone().expect("configure ran");
    assert_eq!(
        received,
        Some(DemoConfig {
            name: Some("hello".into()),
            count: Some(5),
        })
    );
}

#[r2e_core::test]
async fn plugin_config_absent_section_is_none() {
    // Config loaded, but no key lives under the `demo` prefix → None.
    let sink = Arc::new(Mutex::new(None));
    let config = r2e_core::R2eConfig::from_yaml_str("other:\n  key: 1\n").unwrap();
    let _app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(ConfigReadingPlugin { sink: sink.clone() })
        .build_state()
        .await;

    assert_eq!(
        *sink.lock().unwrap(),
        Some(None),
        "absent section yields None"
    );
}

#[r2e_core::test]
async fn plugin_config_no_config_loaded_is_none() {
    // No `load_config` / `with_config` at all → None (the stringly escape hatch
    // is unavailable too, but typed Config degrades gracefully to None).
    let sink = Arc::new(Mutex::new(None));
    let _app = AppBuilder::new()
        .plugin(ConfigReadingPlugin { sink: sink.clone() })
        .build_state()
        .await;

    assert_eq!(
        *sink.lock().unwrap(),
        Some(None),
        "no config loaded yields None"
    );
}

#[r2e_core::test]
#[should_panic(expected = "Invalid configuration for plugin")]
async fn plugin_config_malformed_section_panics_at_boot() {
    // `demo.port` is a string where the section requires an `i64` — the same
    // shape as a malformed controller `#[config]` value. Boot must fail with a
    // validation error naming the plugin and section.
    let config = r2e_core::R2eConfig::from_yaml_str("demo:\n  port: not-a-number\n").unwrap();
    let _app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(StrictConfigPlugin)
        .build_state()
        .await;
}

// ── Provided-bean lifecycle: plugin post-construct / pre-destroy (Phase 5) ──

type LifecycleLog = Arc<Mutex<Vec<&'static str>>>;

/// A plugin-provided bean opting into a post-construct hook.
#[derive(Clone)]
struct InitBean {
    log: LifecycleLog,
}

impl PostConstruct for InitBean {
    fn post_construct(&self) -> r2e_core::lifecycle::LifecycleFuture<'_> {
        Box::pin(async move {
            self.log.lock().unwrap().push("bean-post-construct");
            Ok(())
        })
    }
}

/// Provides `InitBean` and opts it into a post-construct hook via the install
/// context.
struct PostConstructPlugin {
    log: LifecycleLog,
}

impl PreStatePlugin for PostConstructPlugin {
    type Provided = (InitBean,);
    type Deps = ();
    type Config = ();

    fn install(&mut self, ctx: &mut PluginInstallContext<'_>) -> (InitBean,) {
        ctx.run_post_construct::<InitBean>();
        (InitBean {
            log: self.log.clone(),
        },)
    }
}

#[r2e_core::test]
async fn plugin_run_post_construct_fires_at_build_state() {
    let log: LifecycleLog = Arc::new(Mutex::new(Vec::new()));
    let _app = AppBuilder::new()
        .plugin(PostConstructPlugin { log: log.clone() })
        .build_state()
        .await;

    assert_eq!(*log.lock().unwrap(), vec!["bean-post-construct"]);
}

/// A plugin-provided bean with a disposal hook.
#[derive(Clone)]
struct DisposeBean {
    log: LifecycleLog,
}

impl PreDestroy for DisposeBean {
    fn pre_destroy(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            self.log.lock().unwrap().push("bean-dispose");
        })
    }
}

/// Provides `DisposeBean`, opts it into disposal, and also registers a plugin
/// async shutdown hook so we can observe the documented ordering.
struct DisposePlugin {
    log: LifecycleLog,
}

impl PreStatePlugin for DisposePlugin {
    type Provided = (DisposeBean,);
    type Deps = ();
    type Config = ();

    fn install(&mut self, ctx: &mut PluginInstallContext<'_>) -> (DisposeBean,) {
        let log = self.log.clone();
        ctx.on_shutdown_async(move || {
            let log = log.clone();
            async move {
                log.lock().unwrap().push("plugin-async-shutdown");
            }
        });
        ctx.run_pre_destroy::<DisposeBean>();
        (DisposeBean {
            log: self.log.clone(),
        },)
    }
}

#[r2e_core::test]
async fn plugin_pre_destroy_runs_on_shutdown_after_plugin_hooks() {
    let log: LifecycleLog = Arc::new(Mutex::new(Vec::new()));
    let app = AppBuilder::new()
        .plugin(DisposePlugin { log: log.clone() })
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

    stop.stop();
    let result = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server did not stop within 5s")
        .expect("server task panicked");
    assert!(result.is_ok(), "run() returned an error: {result:?}");

    // Bean disposers run within the async shutdown phase, after the plugin's
    // own async shutdown hooks.
    assert_eq!(
        *log.lock().unwrap(),
        vec!["plugin-async-shutdown", "bean-dispose"]
    );
}

// ── Conditional plugins: `<prefix>.enabled` gate (Phase 6) ───────────────────

/// A plugin with a `CONFIG_PREFIX` that provides a bean AND performs post-state
/// sugar (a route + stored data) plus a `configure` that stores more data. Used
/// to prove that `<prefix>.enabled = false` skips the post-state effects while
/// the `Provided` bean survives in the graph.
struct GatedPlugin;

/// Data deposited by `GatedPlugin`'s `configure` (distinct from `StoredData`).
struct GatedConfigured(u32);

impl PreStatePlugin for GatedPlugin {
    type Provided = (Alpha,);
    type Deps = ();
    type Config = ();
    const CONFIG_PREFIX: Option<&'static str> = Some("gated");

    fn install(&mut self, ctx: &mut PluginInstallContext<'_>) -> (Alpha,) {
        ctx.store_data(StoredData(1));
        ctx.add_layer(|router| router.route("/gated", get(|| async { "gated-ok" })));
        (Alpha(99),)
    }

    fn configure(self, _p: &(Alpha,), (): (), _config: Option<()>, ctx: &mut DeferredContext<'_>) {
        ctx.store_data(GatedConfigured(2));
    }
}

#[r2e_core::test]
async fn plugin_enabled_true_by_default_runs_all_effects() {
    // No `gated.enabled` key at all → defaults to enabled: sugar + configure run.
    let config = r2e_core::R2eConfig::from_yaml_str("gated:\n  other: 1\n").unwrap();
    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(GatedPlugin)
        .build_state()
        .await;

    // Provided bean present.
    assert_eq!(app.state().get::<Alpha>(), Alpha(99));
    // Sugar store_data + configure store_data both landed.
    assert_eq!(app.get_plugin_data::<StoredData>().map(|d| d.0), Some(1));
    assert_eq!(
        app.get_plugin_data::<GatedConfigured>().map(|d| d.0),
        Some(2)
    );
    // Sugar route reachable.
    let (status, body) = get_route(app.build(), "/gated").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "gated-ok");
}

#[r2e_core::test]
async fn plugin_enabled_false_skips_effects_but_keeps_beans() {
    let config = r2e_core::R2eConfig::from_yaml_str("gated:\n  enabled: false\n").unwrap();
    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(GatedPlugin)
        .build_state()
        .await;

    // The Provided bean STILL exists — type-level provision list is fixed at
    // compile time; disabling a plugin never removes its beans.
    assert_eq!(app.state().get::<Alpha>(), Alpha(99));
    // But no post-state effects: neither sugar nor configure store_data landed.
    assert_eq!(app.get_plugin_data::<StoredData>().map(|d| d.0), None);
    assert_eq!(app.get_plugin_data::<GatedConfigured>().map(|d| d.0), None);
    // …and the sugar route is absent.
    let (status, _body) = get_route(app.build(), "/gated").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[r2e_core::test]
async fn plugin_enabled_false_without_config_loaded_is_enabled() {
    // No config loaded at all → the gate can't see `gated.enabled`, so the
    // plugin defaults to enabled and all effects run.
    let app = AppBuilder::new().plugin(GatedPlugin).build_state().await;

    assert_eq!(app.state().get::<Alpha>(), Alpha(99));
    assert_eq!(app.get_plugin_data::<StoredData>().map(|d| d.0), Some(1));
    assert_eq!(
        app.get_plugin_data::<GatedConfigured>().map(|d| d.0),
        Some(2)
    );
}
