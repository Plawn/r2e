//! `PluginInstallContext` sugar: layers, stored data, serve/shutdown hooks.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use r2e_core::http::routing::get;
use r2e_core::http::StatusCode;
use r2e_core::plugin::{plugin_action_name, DeferredAction, PluginInstallContext, PreStatePlugin};
use r2e_core::AppBuilder;

use crate::fixtures::{StoredData, SugarMarker};
use crate::support::send_get as get_route;

// ── PluginInstallContext sugar (Phase 2) ────────────────────────────────────

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
