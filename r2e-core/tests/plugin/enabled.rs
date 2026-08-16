//! The `<prefix>.enabled` conditional-plugin gate under the factory-first
//! contract: `build` ALWAYS runs (with `ctx.enabled() == false`), the plugin
//! returns an inert variant, and its effects are dropped.

use r2e_core::http::routing::get;
use r2e_core::http::StatusCode;
use r2e_core::plugin::{
    PluginBuildContext, PluginBuildError, PluginSetupContext, PreStatePlugin,
};
use r2e_core::type_list::BeanAccess;
use r2e_core::AppBuilder;

use crate::fixtures::{BuildProbe, SetupData, StoredData};
use crate::support::send_get as get_route;

/// The bean a gated plugin contributes: carries the `enabled` flag its build
/// observed, standing in for a real plugin's disabled variant.
#[derive(Clone, Debug, PartialEq)]
struct GatedService {
    enabled: bool,
}

/// A plugin with a `CONFIG_PREFIX` whose build registers effects (a route +
/// stored data). Used to prove `<prefix>.enabled = false` drops the effects
/// while the `Provided` bean survives — as an inert variant.
struct GatedPlugin {
    probe: BuildProbe,
}

impl PreStatePlugin for GatedPlugin {
    type Provided = (GatedService,);
    type Deps = ();
    type Config = ();
    const CONFIG_PREFIX: Option<&'static str> = Some("gated");

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(GatedService,), PluginBuildError> {
        self.probe.mark();
        // Effects are registered unconditionally — the framework drops them
        // when the plugin is disabled.
        ctx.store_data(StoredData(1));
        ctx.add_layer(|router| router.route("/gated", get(|| async { "gated-ok" })));
        Ok((GatedService {
            enabled: ctx.enabled(),
        },))
    }
}

#[r2e_core::test]
async fn plugin_enabled_true_by_default_runs_all_effects() {
    // No `gated.enabled` key at all → defaults to enabled.
    let config = r2e_core::R2eConfig::from_yaml_str("gated:\n  other: 1\n").unwrap();
    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(GatedPlugin {
            probe: BuildProbe::default(),
        })
        .build_state()
        .await;

    // Provided bean present, built with enabled() == true.
    assert_eq!(app.state().get::<GatedService>(), GatedService { enabled: true });
    // Effects landed.
    assert_eq!(app.get_plugin_data::<StoredData>().map(|d| d.0), Some(1));
    let (status, body) = get_route(app.build(), "/gated").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "gated-ok");
}

#[r2e_core::test]
async fn plugin_enabled_false_builds_inert_variant_and_drops_effects() {
    let probe = BuildProbe::default();
    let config = r2e_core::R2eConfig::from_yaml_str("gated:\n  enabled: false\n").unwrap();
    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(GatedPlugin {
            probe: probe.clone(),
        })
        .build_state()
        .await;

    // `build` STILL ran — the type-level provision list is fixed at compile
    // time, so the bean must exist…
    assert!(probe.ran(), "build runs even when disabled");
    // …and it observed enabled() == false (the inert-variant signal).
    assert_eq!(
        app.state().get::<GatedService>(),
        GatedService { enabled: false }
    );
    // But the effects were dropped: no stored data, no route.
    assert_eq!(app.get_plugin_data::<StoredData>().map(|d| d.0), None);
    let (status, _body) = get_route(app.build(), "/gated").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// A plugin that uses BOTH slots: `setup()` deposits the coordination datum
/// other pre-state code needs (Scheduler's `TaskRegistryHandle` is the real
/// one), `build` registers the machinery that actually runs — routes, data,
/// and the cleanup hook for what it constructed.
struct SetupAndBuildPlugin {
    disposed: DisposeProbe,
}

impl PreStatePlugin for SetupAndBuildPlugin {
    type Provided = (GatedService,);
    type Deps = ();
    type Config = ();
    const CONFIG_PREFIX: Option<&'static str> = Some("gated");

    fn setup(&mut self, ctx: &mut PluginSetupContext) {
        // The ONLY thing setup may deposit: an ungated coordination datum.
        // Surface effects are unrepresentable here — `PluginSetupContext` has
        // no `add_layer`/`wrap_router`/`on_serve`/`on_shutdown{_async}`,
        // precisely so a disabled plugin cannot mount a route from setup.
        ctx.store_data(SetupData(7));
    }

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(GatedService,), PluginBuildError> {
        ctx.store_data(StoredData(1));
        ctx.add_layer(|router| router.route("/gated", get(|| async { "gated-ok" })));
        // Cleanup for the resource this build constructed — registered
        // unconditionally, and NOT dropped by the enabled gate.
        let disposed = self.disposed.clone();
        ctx.on_shutdown(move || disposed.mark());
        Ok((GatedService {
            enabled: ctx.enabled(),
        },))
    }
}

#[r2e_core::test]
async fn disabling_a_plugin_never_cancels_its_setup_datum() {
    // `setup()` is the pre-graph coordination hook: whatever it deposits is
    // what OTHER pre-state code reads, before and independently of the plugin
    // doing any work. Gating it on `<prefix>.enabled` is what silently broke
    // `#[scheduled]` collection under `scheduler.enabled = false` — the
    // registry handle vanished and task registration panicked. It is safe to
    // leave ungated only because setup cannot register surface effects at all.
    let config = r2e_core::R2eConfig::from_yaml_str("gated:\n  enabled: false\n").unwrap();
    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(SetupAndBuildPlugin {
            disposed: DisposeProbe::default(),
        })
        .build_state()
        .await;

    // The setup datum survived the gate…
    assert_eq!(
        app.get_plugin_data::<SetupData>().map(|d| d.0),
        Some(7),
        "setup data must exist even when the plugin is disabled"
    );
    // …while the build's surface effects were dropped.
    assert_eq!(app.get_plugin_data::<StoredData>().map(|d| d.0), None);
    let (status, _) = get_route(app.build(), "/gated").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "build route is dropped");
}

/// Observed-once flag for a shutdown hook.
#[derive(Clone, Default)]
struct DisposeProbe(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl DisposeProbe {
    fn mark(&self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    fn fired(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Boot the app, serve on an ephemeral port, stop it, and return once the
/// shutdown sequence has completed.
async fn serve_then_stop<S: Clone + Send + Sync + 'static>(app: r2e_core::AppBuilder<S>) {
    let prepared = app.prepare("127.0.0.1:0");
    let stop = prepared.stop_handle();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = tokio::spawn(async move {
        prepared
            .run_with_listener(listener)
            .await
            .map_err(|e| e.to_string())
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    stop.stop();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), server)
        .await
        .expect("server did not stop within 5s")
        .expect("server task panicked");
    assert!(result.is_ok(), "run() returned an error: {result:?}");
}

#[tokio::test]
async fn a_disabled_plugin_still_disposes_of_what_its_build_constructed() {
    // `build` runs whether or not the plugin is enabled, so whatever it
    // constructed exists and must still be disposed of: cleanup is not a
    // surface effect. (The real-world case: a disabled Executor still builds a
    // pool; dropping its drain hook with the surface effects means it is never
    // drained.)
    let disposed = DisposeProbe::default();
    let config = r2e_core::R2eConfig::from_yaml_str("gated:\n  enabled: false\n").unwrap();
    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(SetupAndBuildPlugin {
            disposed: disposed.clone(),
        })
        .build_state()
        .await;

    serve_then_stop(app).await;
    assert!(
        disposed.fired(),
        "the shutdown hook of a DISABLED plugin must still run"
    );
}

#[tokio::test]
async fn an_enabled_plugin_disposes_too() {
    // The control: same plugin, gate open.
    let disposed = DisposeProbe::default();
    let config = r2e_core::R2eConfig::from_yaml_str("gated:\n  enabled: true\n").unwrap();
    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(SetupAndBuildPlugin {
            disposed: disposed.clone(),
        })
        .build_state()
        .await;

    serve_then_stop(app).await;
    assert!(disposed.fired(), "shutdown hook ran");
}

#[r2e_core::test]
async fn enabled_is_decided_once_and_travels_with_the_effects() {
    // The gate has two potential readers: the group factory (the graph's
    // `R2eConfig` bean) and the install-order deferred action (the builder's
    // own loaded config). A pinned `R2eConfig` makes them disagree — the only
    // acceptable outcome is that ONE decision is taken, in the factory, and
    // carried to the action alongside the effects it governs.
    let pinned = r2e_core::R2eConfig::from_yaml_str("gated:\n  enabled: false\n").unwrap();
    let loaded = r2e_core::R2eConfig::from_yaml_str("gated:\n  enabled: true\n").unwrap();
    let app = AppBuilder::new()
        // Pinned before `load_config`, so the graph sees this one while
        // `DeferredContext::config()` still sees the loaded one.
        .override_bean(pinned)
        .override_config(loaded)
        .load_config::<()>()
        .plugin(GatedPlugin {
            probe: BuildProbe::default(),
        })
        .build_state()
        .await;

    // What `build` observed…
    let observed = app.state().get::<GatedService>().enabled;
    assert!(!observed, "build reads the graph's config: disabled");
    // …must be what the effects did. Disabled ⇒ no data, no route.
    assert_eq!(
        app.get_plugin_data::<StoredData>().map(|d| d.0),
        None,
        "effects must follow the decision build saw ({observed})"
    );
    let (status, _) = get_route(app.build(), "/gated").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "effects must follow the decision build saw ({observed})"
    );
}

#[r2e_core::test]
async fn enabled_is_decided_once_in_the_other_direction_too() {
    // Same setup, opposite disagreement: the graph says enabled, the
    // builder's config says disabled. The plugin builds live, so its effects
    // must apply — recomputing the gate from `DeferredContext` would drop the
    // routes of a plugin that believes it is running.
    let pinned = r2e_core::R2eConfig::from_yaml_str("gated:\n  enabled: true\n").unwrap();
    let loaded = r2e_core::R2eConfig::from_yaml_str("gated:\n  enabled: false\n").unwrap();
    let app = AppBuilder::new()
        .override_bean(pinned)
        .override_config(loaded)
        .load_config::<()>()
        .plugin(GatedPlugin {
            probe: BuildProbe::default(),
        })
        .build_state()
        .await;

    let observed = app.state().get::<GatedService>().enabled;
    assert!(observed, "build reads the graph's config: enabled");
    assert_eq!(app.get_plugin_data::<StoredData>().map(|d| d.0), Some(1));
    let (status, body) = get_route(app.build(), "/gated").await;
    assert_eq!(status, StatusCode::OK, "effects follow build's decision");
    assert_eq!(body, "gated-ok");
}

#[r2e_core::test]
async fn plugin_without_config_loaded_is_enabled() {
    // No config loaded at all → the gate can't see `gated.enabled`, so the
    // plugin defaults to enabled and all effects run.
    let app = AppBuilder::new()
        .plugin(GatedPlugin {
            probe: BuildProbe::default(),
        })
        .build_state()
        .await;

    assert_eq!(app.state().get::<GatedService>(), GatedService { enabled: true });
    assert_eq!(app.get_plugin_data::<StoredData>().map(|d| d.0), Some(1));
}
