//! `Deps` resolution and the `configure` hook.

use std::sync::{Arc, Mutex};

use r2e_core::beans::{Bean, BeanContext, BeanRegistry, Registrable};
use r2e_core::http::routing::get;
use r2e_core::http::StatusCode;
use r2e_core::plugin::{DeferredContext, PluginInstallContext, PreStatePlugin};
use r2e_core::type_list::BeanAccess;
use r2e_core::{AppBuilder, TNil};
use std::any::TypeId;

use crate::fixtures::{Alpha, StoredData, SugarMarker};
use crate::support::send_get as get_route;

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
