//! Builder-level test-harness pre-configuration: `override_bean`,
//! `override_config_value`, `with_profile`.

use r2e_core::config::{ConfigValue, R2eConfig};
use r2e_core::AppBuilder;

#[derive(Clone, PartialEq, Debug)]
struct Greeter {
    origin: &'static str,
}

#[r2e_core::test]
async fn override_bean_wins_over_later_provide() {
    let builder = AppBuilder::new()
        .override_bean(Greeter { origin: "pinned" })
        .provide(Greeter { origin: "real" })
        .build_state()
        .await;

    let greeter = builder.bean_context().get::<Greeter>();
    assert_eq!(greeter.origin, "pinned");
}

#[r2e_core::test]
async fn override_config_value_before_with_config() {
    let mut config = R2eConfig::empty();
    config.set("app.greeting", ConfigValue::String("prod".into()));

    let builder = AppBuilder::new()
        .override_config_value("app.greeting", "patched")
        .override_config_value("app.port", 8081)
        .with_config(config)
        .build_state()
        .await;

    let config = builder.bean_context().get::<R2eConfig>();
    assert_eq!(config.get::<String>("app.greeting").unwrap(), "patched");
    assert_eq!(config.get::<i64>("app.port").unwrap(), 8081);
}

#[r2e_core::test]
async fn override_config_value_after_with_config() {
    let mut config = R2eConfig::empty();
    config.set("app.greeting", ConfigValue::String("prod".into()));

    let builder = AppBuilder::new()
        .with_config(config)
        .override_config_value("app.greeting", "patched")
        .build_state()
        .await;

    let config = builder.bean_context().get::<R2eConfig>();
    assert_eq!(config.get::<String>("app.greeting").unwrap(), "patched");
}

#[r2e_core::test]
async fn with_profile_forces_active_profile() {
    let builder = AppBuilder::new().with_profile("test");
    assert_eq!(builder.active_profile(), "test");
    assert!(builder.profile_is("test"));

    // The forced profile survives a with_config that would otherwise resolve
    // the profile from the config/env.
    let mut config = R2eConfig::empty();
    config.set("r2e.profile", ConfigValue::String("prod".into()));
    let builder = AppBuilder::new().with_profile("test").with_config(config);
    assert_eq!(builder.active_profile(), "test");
}

#[test]
fn load_profiled_records_explicit_profile() {
    // No application.yaml in the test cwd — the explicit profile must still
    // be recorded on the r2e.profile key.
    let config = R2eConfig::load_profiled(Some("test")).unwrap();
    assert_eq!(config.get::<String>("r2e.profile").unwrap(), "test");
}
