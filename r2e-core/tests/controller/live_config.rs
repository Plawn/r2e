//! Controller `#[live_config("key")]` fields.
//!
//! App-scoped exactly like `#[config]` — resolved once at registration onto the
//! controller core — but the *value* is live: `handle.get()` re-reads the
//! `LiveConfigRegistry` on every call, so a runtime provider update changes the
//! response without rebuilding the controller.

use r2e_core::config::{
    ConfigKeyKind, ConfigUpdateSink, ConfigValue, LiveConfig, LiveConfigRegistry, R2eConfig,
};
use r2e_core::http::StatusCode;
use r2e_core::prelude::*;
use r2e_core::{AppBuilder, BeanAccess, ContextConstruct};

use crate::support::send_get;

#[controller(path = "/live")]
pub struct LiveConfigController {
    #[live_config("app.banner")]
    banner: LiveConfig<String>,
    #[config("app.name")]
    name: String,
}

#[routes]
impl LiveConfigController {
    #[get("/")]
    async fn banner(&self) -> String {
        format!(
            "{}: {}",
            self.name,
            self.banner.get().unwrap_or_else(|_| "<unset>".into())
        )
    }
}

fn config_with_banner() -> R2eConfig {
    let mut config = R2eConfig::empty();
    config.set("app.banner", ConfigValue::String("hello".into()));
    config.set("app.name", ConfigValue::String("demo".into()));
    config
}

async fn get_banner(router: &r2e_core::http::Router) -> String {
    let (status, body) = send_get(router.clone(), "/live").await;
    assert_eq!(status, StatusCode::OK);
    body
}

#[r2e_core::test]
async fn controller_live_config_field_reads_boot_value_and_runtime_updates() {
    let app = AppBuilder::new()
        .override_config(config_with_banner())
        .load_config::<()>()
        .build_state()
        .await;
    let registry = app.state().get::<LiveConfigRegistry>();
    let router = app.register_controller::<LiveConfigController>().build();

    assert_eq!(get_banner(&router).await, "demo: hello");

    // The core was built once at registration; the value still moves.
    let sink = ConfigUpdateSink::new(registry);
    assert!(sink.set("app.banner", "rotated"));
    assert_eq!(get_banner(&router).await, "demo: rotated");
}

/// A live key absent at boot must not fail registration (it is never
/// presence-validated) — the handle reports it at `get()` time until a runtime
/// write lands.
#[r2e_core::test]
async fn controller_live_config_field_tolerates_missing_key_at_boot() {
    let mut config = R2eConfig::empty();
    config.set("app.name", ConfigValue::String("demo".into()));

    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .build_state()
        .await;
    let registry = app.state().get::<LiveConfigRegistry>();
    let router = app.register_controller::<LiveConfigController>().build();

    assert_eq!(get_banner(&router).await, "demo: <unset>");

    assert!(registry.set("app.banner", "late"));
    assert_eq!(get_banner(&router).await, "demo: late");
}

/// The controller core reports its config keys like a bean does: the live key
/// is present with kind `Live` (subscribed — neither presence-validated nor
/// fingerprinted), while a plain `#[config]` key stays `Required` (copied).
#[test]
fn controller_live_config_key_is_subscribed_not_copied() {
    let keys = <LiveConfigController as ContextConstruct>::config_keys();

    let live = keys
        .iter()
        .find(|(key, _, _)| *key == "app.banner")
        .expect("live_config key must appear in config_keys()");
    assert_eq!(live.2, ConfigKeyKind::Live);
    assert!(!live.2.is_required());
    assert!(!live.2.is_fingerprinted());
    assert_eq!(live.1, "LiveConfig < String >");

    let plain = keys
        .iter()
        .find(|(key, _, _)| *key == "app.name")
        .expect("plain config key must still appear");
    assert_eq!(plain.2, ConfigKeyKind::Required);
    assert!(plain.2.is_fingerprinted());
}
