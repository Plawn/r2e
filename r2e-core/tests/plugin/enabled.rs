//! The `<prefix>.enabled` conditional-plugin gate under the factory-first
//! contract: `build` ALWAYS runs (with `ctx.enabled() == false`), the plugin
//! returns an inert variant, and its effects are dropped.

use r2e_core::http::routing::get;
use r2e_core::http::StatusCode;
use r2e_core::plugin::{PluginBuildContext, PluginBuildError, PreStatePlugin};
use r2e_core::type_list::BeanAccess;
use r2e_core::AppBuilder;

use crate::fixtures::{BuildProbe, StoredData};
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
