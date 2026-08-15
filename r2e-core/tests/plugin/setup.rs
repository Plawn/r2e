//! `PluginSetupContext` — the rare pre-graph escape hatch: buffered sugar,
//! explicit `add_deferred`, and the documented per-plugin action ordering
//! `[explicit…, setup-sugar, build-effects]`.

use std::time::Duration;

use r2e_core::http::routing::get;
use r2e_core::http::StatusCode;
use r2e_core::plugin::{
    plugin_action_name, DeferredAction, PluginBuildContext, PluginBuildError, PluginSetupContext,
    PreStatePlugin,
};
use r2e_core::AppBuilder;

use crate::fixtures::{EventLog, StoredData, SugarMarker};
use crate::support::send_get as get_route;

/// A plugin that reaches only for the buffered setup sugar — no
/// `DeferredAction`, no build effects.
struct SugarSetupPlugin;

impl PreStatePlugin for SugarSetupPlugin {
    type Provided = (SugarMarker,);
    type Deps = ();
    type Config = ();

    fn setup(&mut self, ctx: &mut PluginSetupContext) {
        ctx.store_data(StoredData(42));
        ctx.add_layer(|router| router.route("/sugar", get(|| async { "sugar-ok" })));
        ctx.wrap_router(|router| router.route("/wrapped", get(|| async { "wrapped-ok" })));
    }

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        _ctx: &mut PluginBuildContext,
    ) -> Result<(SugarMarker,), PluginBuildError> {
        Ok((SugarMarker,))
    }
}

#[r2e_core::test]
async fn setup_sugar_layers_and_data_land_and_execute() {
    let app = AppBuilder::new()
        .plugin(SugarSetupPlugin)
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

/// Exercises all three per-plugin action slots — explicit `add_deferred`,
/// setup sugar, and build effects — so the documented ordering
/// `[explicit…, setup-sugar, build-effects]` is observable end-to-end.
struct EveryHookPlugin {
    log: EventLog,
}

impl PreStatePlugin for EveryHookPlugin {
    type Provided = (SugarMarker,);
    type Deps = ();
    type Config = ();

    fn setup(&mut self, ctx: &mut PluginSetupContext) {
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
        ctx.on_serve(move |_sc| l_ss.push("setup-serve"));
        let l_ssh = log.clone();
        ctx.on_shutdown(move || l_ssh.push("setup-shutdown"));
        let l_sa = log;
        ctx.on_shutdown_async(move || async move { l_sa.push("setup-async-shutdown") });
    }

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(SugarMarker,), PluginBuildError> {
        // Build effects land in the third slot, after setup's actions.
        let l_bs = self.log.clone();
        ctx.on_serve(move |_sc| l_bs.push("build-serve"));
        Ok((SugarMarker,))
    }
}

#[tokio::test]
async fn setup_actions_execute_in_documented_order() {
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
    let pos = |e: &str| entries.iter().position(|x| *x == e);

    // Serve hooks executed in slot order: explicit → setup sugar → build.
    let es = pos("explicit-serve").expect("explicit serve hook ran");
    let ss = pos("setup-serve").expect("setup sugar serve hook ran");
    let bs = pos("build-serve").expect("build-effect serve hook ran");
    assert!(es < ss, "explicit before setup sugar: {entries:?}");
    assert!(ss < bs, "setup sugar before build effects: {entries:?}");

    // Shutdown hooks (sync + async) executed; explicit before sugar.
    let esh = pos("explicit-shutdown").expect("explicit shutdown ran");
    let ssh = pos("setup-shutdown").expect("setup shutdown ran");
    assert!(esh < ssh, "explicit shutdown before setup shutdown: {entries:?}");
    assert!(
        entries.contains(&"setup-async-shutdown"),
        "async shutdown hook ran: {entries:?}"
    );
}

#[test]
fn plugin_action_name_trims_to_last_segment() {
    // A path-qualified type collapses to its final segment…
    assert_eq!(plugin_action_name::<SugarSetupPlugin>(), "SugarSetupPlugin");
    // …and a primitive with no path is returned as-is.
    assert_eq!(plugin_action_name::<u32>(), "u32");
}
