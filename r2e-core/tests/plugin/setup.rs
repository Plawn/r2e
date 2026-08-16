//! `PluginSetupContext` — the rare pre-graph escape hatch: the buffered
//! `store_data` datum, explicit `add_deferred`, and the documented per-plugin
//! action ordering `[explicit…, setup-data, build-effects]`.
//!
//! Setup deliberately has **no** effect sugar (no `add_layer`, `wrap_router`,
//! `on_serve`, `on_shutdown{_async}`): everything it registers runs
//! unconditionally, so surface effects — which must vanish under
//! `<prefix>.enabled = false` — live on `PluginBuildContext` instead. See
//! `enabled.rs` for the gating tests.

use std::time::Duration;

use r2e_core::http::routing::get;
use r2e_core::http::StatusCode;
use r2e_core::plugin::{
    plugin_action_name, DeferredAction, PluginBuildContext, PluginBuildError, PluginSetupContext,
    PreStatePlugin,
};
use r2e_core::AppBuilder;

use crate::fixtures::{EventLog, SetupData, StoredData, SugarMarker};
use crate::support::send_get as get_route;

/// A plugin that deposits a pre-graph coordination datum in `setup` and mounts
/// its routes from `build`, where the enabled gate applies.
struct SugarSetupPlugin;

impl PreStatePlugin for SugarSetupPlugin {
    type Provided = (SugarMarker,);
    type Deps = ();
    type Config = ();

    fn setup(&mut self, ctx: &mut PluginSetupContext) {
        ctx.store_data(SetupData(42));
    }

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(SugarMarker,), PluginBuildError> {
        ctx.store_data(StoredData(7));
        ctx.add_layer(|router| router.route("/sugar", get(|| async { "sugar-ok" })));
        ctx.wrap_router(|router| router.route("/wrapped", get(|| async { "wrapped-ok" })));
        Ok((SugarMarker,))
    }
}

#[r2e_core::test]
async fn setup_data_lands_and_build_effects_apply() {
    let app = AppBuilder::new()
        .plugin(SugarSetupPlugin)
        .build_state()
        .await;

    // `setup`'s `store_data` was flushed into plugin_data at build_state…
    assert_eq!(app.get_plugin_data::<SetupData>().map(|d| d.0), Some(42));
    // …and so was `build`'s.
    assert_eq!(app.get_plugin_data::<StoredData>().map(|d| d.0), Some(7));

    // `add_layer` and `wrap_router` build effects produced reachable routes.
    let router = app.build();
    let (status, body) = get_route(router.clone(), "/sugar").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "sugar-ok");
    let (status, body) = get_route(router, "/wrapped").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "wrapped-ok");
}

/// Exercises all three per-plugin action slots — explicit `add_deferred`,
/// setup's buffered `store_data`, and build effects — so the documented
/// ordering `[explicit…, setup-data, build-effects]` is observable end-to-end.
struct EveryHookPlugin {
    log: EventLog,
}

impl PreStatePlugin for EveryHookPlugin {
    type Provided = (SugarMarker,);
    type Deps = ();
    type Config = ();

    fn setup(&mut self, ctx: &mut PluginSetupContext) {
        let log = self.log.clone();

        // Escape hatch: explicit actions run BEFORE the buffered data action —
        // which is exactly why `take_data` here must NOT yet see `SetupData`.
        let l_ex = log.clone();
        let l_es = log.clone();
        let l_esh = log.clone();
        ctx.add_deferred(DeferredAction::new("explicit", move |dctx| {
            if dctx.take_data::<SetupData>().is_some() {
                l_ex.push("explicit-saw-setup-data");
            } else {
                l_ex.push("explicit-before-setup-data");
            }
            dctx.on_serve(move |_sc| l_es.push("explicit-serve"));
            dctx.on_shutdown(move || l_esh.push("explicit-shutdown"));
        }));

        // The buffered datum: second slot.
        ctx.store_data(SetupData(1));
    }

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(SugarMarker,), PluginBuildError> {
        // Build effects land in the third slot, after setup's actions — so this
        // one DOES see the datum setup deposited.
        let l_ab = self.log.clone();
        ctx.after_build(move |dctx| {
            if dctx.take_data::<SetupData>().is_some() {
                l_ab.push("build-saw-setup-data");
            } else {
                l_ab.push("build-missed-setup-data");
            }
        });
        let l_bs = self.log.clone();
        ctx.on_serve(move |_sc| l_bs.push("build-serve"));
        let l_bsh = self.log.clone();
        ctx.on_shutdown(move || l_bsh.push("build-shutdown"));
        let l_ba = self.log.clone();
        ctx.on_shutdown_async(move || async move { l_ba.push("build-async-shutdown") });
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

    // Deferred slots ran in order: the explicit action could not yet see the
    // datum the buffered setup action deposits, the build effect could.
    assert!(
        entries.contains(&"explicit-before-setup-data"),
        "explicit action ran before the setup-data action: {entries:?}"
    );
    assert!(
        entries.contains(&"build-saw-setup-data"),
        "build effects ran after the setup-data action: {entries:?}"
    );

    // Serve hooks executed in slot order: explicit → build.
    let es = pos("explicit-serve").expect("explicit serve hook ran");
    let bs = pos("build-serve").expect("build-effect serve hook ran");
    assert!(es < bs, "explicit before build effects: {entries:?}");

    // Shutdown hooks (sync + async) executed; explicit before build.
    let esh = pos("explicit-shutdown").expect("explicit shutdown ran");
    let bsh = pos("build-shutdown").expect("build shutdown ran");
    assert!(
        esh < bsh,
        "explicit shutdown before build shutdown: {entries:?}"
    );
    assert!(
        entries.contains(&"build-async-shutdown"),
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
