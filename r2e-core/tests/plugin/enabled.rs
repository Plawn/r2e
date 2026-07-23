//! The `<prefix>.enabled` conditional-plugin gate.

use r2e_core::http::routing::get;
use r2e_core::http::StatusCode;
use r2e_core::plugin::{DeferredContext, PluginInstallContext, PreStatePlugin};
use r2e_core::type_list::BeanAccess;
use r2e_core::AppBuilder;

use crate::fixtures::{Alpha, StoredData};
use crate::support::send_get as get_route;

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
