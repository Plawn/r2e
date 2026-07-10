//! `TestApp::boot` / `boot_with`: blueprint booting, pinned mocks, config
//! overrides, bean access.

use r2e_core::config::{ConfigValue, R2eConfig};
use r2e_core::{AppBuilder, BootableApp};
use r2e_test::TestApp;

#[derive(Clone, Debug, PartialEq)]
struct Greeter {
    origin: &'static str,
}

/// A minimal blueprint, shaped like an app's `lib.rs` assembly function.
async fn blueprint(b: AppBuilder) -> impl BootableApp {
    let mut config = R2eConfig::empty();
    config.set("app.greeting", ConfigValue::String("prod".into()));
    b.with_config(config)
        .provide(Greeter { origin: "real" })
        .build_state()
        .await
}

#[tokio::test]
async fn boot_exposes_beans_and_config() {
    let app = TestApp::boot(blueprint).await;

    assert_eq!(app.bean::<Greeter>(), Greeter { origin: "real" });
    assert_eq!(app.config().get::<String>("app.greeting").unwrap(), "prod");
}

#[tokio::test]
async fn boot_with_pins_mocks_over_app_beans() {
    let app = TestApp::boot_with(blueprint, |b| {
        b.override_bean(Greeter { origin: "mock" })
    })
    .await;

    assert_eq!(app.bean::<Greeter>(), Greeter { origin: "mock" });
}

#[tokio::test]
async fn boot_with_patches_config_keys() {
    let app = TestApp::boot_with(blueprint, |b| {
        b.override_config_value("app.greeting", "patched")
    })
    .await;

    assert_eq!(
        app.config().get::<String>("app.greeting").unwrap(),
        "patched"
    );
}

#[tokio::test]
async fn boot_forces_test_profile() {
    // The profile is surfaced on the builder (with_config path does not
    // rewrite the config) — record it from inside the blueprint.
    let app = TestApp::boot(|b: AppBuilder| async move {
        let profile = b.active_profile().to_string();
        b.provide(profile).build_state().await
    })
    .await;
    assert_eq!(app.bean::<String>(), "test");
}

#[tokio::test]
async fn boot_wires_a_test_jwt() {
    let app = TestApp::boot(blueprint).await;
    // The TestJwt is available and mints parseable tokens.
    let token = app.test_jwt().token("alice", &["admin"]);
    assert_eq!(token.matches('.').count(), 2, "expected a JWT-shaped token");
}
