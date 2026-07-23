//! Typed plugin `Config` / `CONFIG_PREFIX` loading.

use std::sync::{Arc, Mutex};

use r2e_core::plugin::{DeferredContext, PluginInstallContext, PreStatePlugin};
use r2e_core::AppBuilder;

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
