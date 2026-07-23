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
async fn override_config_provides_in_memory_config() {
    // A key that lives in no YAML on disk must still resolve — proving
    // load_config consumed the in-memory config instead of reading a file.
    let mut config = R2eConfig::empty();
    config.set(
        "app.only_in_memory",
        ConfigValue::String("from-memory".into()),
    );

    let builder = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .build_state()
        .await;

    let config = builder.bean_context().get::<R2eConfig>();
    assert_eq!(
        config.get::<String>("app.only_in_memory").unwrap(),
        "from-memory"
    );
}

#[r2e_core::test]
async fn override_config_value_before_override_config() {
    let mut config = R2eConfig::empty();
    config.set("app.greeting", ConfigValue::String("prod".into()));

    let builder = AppBuilder::new()
        .override_config_value("app.greeting", "patched")
        .override_config_value("app.port", 8081)
        .override_config(config)
        .load_config::<()>()
        .build_state()
        .await;

    let config = builder.bean_context().get::<R2eConfig>();
    // override_config_value wins over override_config regardless of order.
    assert_eq!(config.get::<String>("app.greeting").unwrap(), "patched");
    assert_eq!(config.get::<i64>("app.port").unwrap(), 8081);
}

#[r2e_core::test]
async fn override_config_value_after_override_config() {
    let mut config = R2eConfig::empty();
    config.set("app.greeting", ConfigValue::String("prod".into()));

    let builder = AppBuilder::new()
        .override_config(config)
        .override_config_value("app.greeting", "patched")
        .load_config::<()>()
        .build_state()
        .await;

    let config = builder.bean_context().get::<R2eConfig>();
    // override_config_value still wins even when set after override_config.
    assert_eq!(config.get::<String>("app.greeting").unwrap(), "patched");
}

#[r2e_core::test]
async fn with_profile_forces_active_profile() {
    let builder = AppBuilder::new().with_profile("test");
    assert_eq!(builder.active_profile(), "test");
    assert!(builder.profile_is("test"));

    // The forced profile survives a load_config (via override_config) that
    // would otherwise resolve the profile from the config/env.
    let mut config = R2eConfig::empty();
    config.set("r2e.profile", ConfigValue::String("prod".into()));
    let builder = AppBuilder::new()
        .with_profile("test")
        .override_config(config)
        .load_config::<()>();
    assert_eq!(builder.active_profile(), "test");
}

#[test]
fn load_profiled_records_explicit_profile() {
    // No application.yaml in the test cwd — the explicit profile must still
    // be recorded on the r2e.profile key.
    let config = R2eConfig::load_profiled(Some("test")).unwrap();
    assert_eq!(config.get::<String>("r2e.profile").unwrap(), "test");
}

#[r2e_core::test]
async fn with_config_file_loads_custom_base_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("patina.yaml");
    std::fs::write(&file, "app:\n  name: patina\n").unwrap();

    let builder = AppBuilder::new()
        .with_config_file(&file)
        .override_config_value("app.port", 8081)
        .load_config::<()>()
        .build_state()
        .await;

    let config = builder.bean_context().get::<R2eConfig>();
    assert_eq!(config.get::<String>("app.name").unwrap(), "patina");
    assert_eq!(config.get::<i64>("app.port").unwrap(), 8081);
}

#[r2e_core::test]
async fn with_config_file_and_profile_overlays_derived_sibling() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("patina.yaml"), "app:\n  port: 9000\n").unwrap();
    std::fs::write(dir.path().join("patina-test.yaml"), "app:\n  port: 1234\n").unwrap();

    let builder = AppBuilder::new()
        .with_profile("test")
        .with_config_file(dir.path().join("patina.yaml"))
        .load_config::<()>()
        .build_state()
        .await;

    let config = builder.bean_context().get::<R2eConfig>();
    assert_eq!(config.get::<String>("r2e.profile").unwrap(), "test");
    assert_eq!(config.get::<i64>("app.port").unwrap(), 1234);
}

#[r2e_core::test]
#[should_panic(expected = "load_config() was never called")]
async fn override_config_without_load_config_panics_at_build_state() {
    // override_config stashes a config but nothing consumes it — build_state
    // must catch the silent-ignore mistake.
    let _ = AppBuilder::new()
        .override_config(R2eConfig::empty())
        .build_state()
        .await;
}

#[r2e_core::test]
#[should_panic(expected = "mutually exclusive")]
async fn override_config_with_config_file_panics() {
    // override_config + with_config_file can't both be honored — load_config
    // panics when it sees both.
    let _ = AppBuilder::new()
        .with_config_file("patina.yaml")
        .override_config(R2eConfig::empty())
        .load_config::<()>()
        .build_state()
        .await;
}

#[r2e_core::test]
#[should_panic(expected = "after load_config()")]
async fn override_config_after_load_config_panics() {
    // The stash could never be consumed — fail at the call site with a
    // message naming the real fault (wrong order), not a missing load_config.
    let _ = AppBuilder::new()
        .load_config::<()>()
        .override_config(R2eConfig::empty());
}

#[r2e_core::test]
#[should_panic(expected = "override_config_value() was set but load_config() was never called")]
async fn override_config_value_without_load_config_panics_at_build_state() {
    // A stashed key/value with no load_config to drain it would be silently
    // ignored — build_state must catch it like the override_config case.
    let _ = AppBuilder::new()
        .override_config_value("app.x", ConfigValue::from("y"))
        .build_state()
        .await;
}

#[r2e_core::test]
#[should_panic(expected = "with_config_file() was set but load_config() was never called")]
async fn with_config_file_without_load_config_panics_at_build_state() {
    // A config file that no load_config ever reads would be silently ignored.
    let _ = AppBuilder::new()
        .with_config_file("patina.yaml")
        .build_state()
        .await;
}
