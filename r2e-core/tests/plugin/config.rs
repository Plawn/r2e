//! Typed plugin `Config` / `CONFIG_PREFIX` loading, delivered to `build`.

use std::sync::{Arc, Mutex};

use r2e_core::plugin::{PluginBuildContext, PluginBuildError, PreStatePlugin};
use r2e_core::AppBuilder;

/// An all-optional config section, so its presence — not any required key —
/// drives whether `build` gets `Some`.
#[derive(r2e_core::prelude::ConfigProperties, Clone, Debug, Default, PartialEq)]
struct DemoConfig {
    name: Option<String>,
    count: Option<i64>,
}

/// A config section with a **required** field, used to exercise validation.
/// The field is only ever read by the derived validator — boot fails before
/// any test code could touch it.
#[derive(r2e_core::prelude::ConfigProperties, Clone, Debug)]
struct StrictConfig {
    #[allow(dead_code)]
    port: i64,
}

/// Records the `Option<Config>` its `build` receives, so tests can assert on
/// the presence/values the framework delivered.
struct ConfigReadingPlugin {
    sink: Arc<Mutex<Option<Option<DemoConfig>>>>,
}

impl PreStatePlugin for ConfigReadingPlugin {
    type Provided = ();
    type Deps = ();
    type Config = DemoConfig;
    const CONFIG_PREFIX: Option<&'static str> = Some("demo");

    async fn build(
        self,
        _deps: (),
        config: Option<DemoConfig>,
        _ctx: &mut PluginBuildContext,
    ) -> Result<(), PluginBuildError> {
        *self.sink.lock().unwrap() = Some(config);
        Ok(())
    }
}

/// A plugin whose `build` must never run because validation panics first.
struct StrictConfigPlugin;

impl PreStatePlugin for StrictConfigPlugin {
    type Provided = ();
    type Deps = ();
    type Config = StrictConfig;
    const CONFIG_PREFIX: Option<&'static str> = Some("demo");

    async fn build(
        self,
        _deps: (),
        _config: Option<StrictConfig>,
        _ctx: &mut PluginBuildContext,
    ) -> Result<(), PluginBuildError> {
        Ok(())
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

    let received = sink.lock().unwrap().clone().expect("build ran");
    assert_eq!(
        received,
        Some(DemoConfig {
            name: Some("hello".into()),
            count: Some(5),
        })
    );
}

#[r2e_core::test]
async fn plugin_installed_before_load_config_still_gets_config() {
    // Order-independence: `.plugin()` BEFORE `load_config` reads the same
    // config — `R2eConfig` is a graph bean available to every build factory,
    // so install/config ordering no longer matters.
    let sink = Arc::new(Mutex::new(None));
    let config = r2e_core::R2eConfig::from_yaml_str("demo:\n  count: 9\n").unwrap();
    let _app = AppBuilder::new()
        .plugin(ConfigReadingPlugin { sink: sink.clone() })
        .override_config(config)
        .load_config::<()>()
        .build_state()
        .await;

    let received = sink.lock().unwrap().clone().expect("build ran");
    assert_eq!(
        received,
        Some(DemoConfig {
            name: None,
            count: Some(9),
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
    // No `load_config` / `with_config` at all → None (typed Config degrades
    // gracefully; `ctx.config_raw()` would be None too).
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
