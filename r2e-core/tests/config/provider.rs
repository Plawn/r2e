use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use r2e_core::beans::Producer;
use r2e_core::config::{
    ConfigError, ConfigKeyKind, ConfigProvider, ConfigProviderContext, ConfigUpdateSink,
    ConfigValue, ConfigWatchContext, LiveConfig, LiveConfigRegistry, R2eConfig,
};
use r2e_core::{AppBuilder, BeanAccess};

#[r2e_macros::producer]
fn produce_db_url(#[live_config("db.url")] url: LiveConfig<String>) -> LiveConfig<String> {
    url
}

#[derive(Clone)]
struct StaticProvider {
    key: &'static str,
    value: &'static str,
}

impl ConfigProvider for StaticProvider {
    fn load(
        &self,
        config: &mut R2eConfig,
        _ctx: ConfigProviderContext<'_>,
    ) -> Result<(), ConfigError> {
        config.set(self.key, ConfigValue::String(self.value.to_string()));
        Ok(())
    }
}

#[derive(Clone)]
struct WatchProvider {
    key: &'static str,
    value: &'static str,
}

impl ConfigProvider for WatchProvider {
    fn load(
        &self,
        config: &mut R2eConfig,
        _ctx: ConfigProviderContext<'_>,
    ) -> Result<(), ConfigError> {
        config.set(self.key, ConfigValue::String("initial".to_string()));
        Ok(())
    }

    fn watch(
        self: Arc<Self>,
        _ctx: ConfigWatchContext,
        sink: ConfigUpdateSink,
    ) -> Pin<Box<dyn Future<Output = Result<(), ConfigError>> + Send + 'static>> {
        Box::pin(async move {
            sink.set(self.key, self.value.to_string());
            Ok(())
        })
    }
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct ProviderConfig {
    app_name: String,
}

#[test]
fn live_config_handle_reads_updates() {
    let registry = LiveConfigRegistry::new();
    registry.set("db.url", "postgres://first");

    let handle: LiveConfig<String> = registry.live_config("db.url");
    assert_eq!(handle.get().unwrap(), "postgres://first");

    registry.set("db.url", "postgres://second");
    assert_eq!(handle.get().unwrap(), "postgres://second");
    assert_eq!(handle.snapshot().version(), 2);
}

#[tokio::test]
async fn provider_values_are_visible_to_typed_config_and_live_handles() {
    let app = AppBuilder::new()
        .override_config(R2eConfig::empty())
        .with_config_provider(StaticProvider {
            key: "app_name",
            value: "from-provider",
        })
        .load_config::<ProviderConfig>()
        .build_state()
        .await;

    assert_eq!(
        app.state().get::<ProviderConfig>().app_name,
        "from-provider"
    );
    let registry = app.state().get::<LiveConfigRegistry>();
    assert_eq!(registry.get::<String>("app_name").unwrap(), "from-provider");
}

#[tokio::test]
async fn override_config_value_pins_runtime_live_updates() {
    let app = AppBuilder::new()
        .override_config(R2eConfig::empty())
        .with_config_provider(WatchProvider {
            key: "db.url",
            value: "provider-update",
        })
        .override_config_value("db.url", "test-override")
        .load_config::<()>()
        .build_state()
        .await;

    let registry = app.state().get::<LiveConfigRegistry>();
    let sink = ConfigUpdateSink::new(registry.clone());
    assert!(!sink.set("db.url", "provider-update"));
    assert_eq!(registry.get::<String>("db.url").unwrap(), "test-override");
}

/// `override_config_value` is documented as order-agnostic: called *after*
/// `load_config` it must patch the live registry too, not just `R2eConfig` —
/// otherwise `#[live_config]` handles keep reading the pre-override value.
#[tokio::test]
async fn late_override_config_value_reaches_live_registry() {
    let mut config = R2eConfig::empty();
    config.set("db.url", ConfigValue::String("postgres://boot".to_string()));

    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .override_config_value("db.url", "postgres://late-override")
        .build_state()
        .await;

    let registry = app.state().get::<LiveConfigRegistry>();
    assert_eq!(
        registry.get::<String>("db.url").unwrap(),
        "postgres://late-override"
    );
    // The raw config bean stays in sync with the live slot.
    assert_eq!(
        app.state()
            .get::<R2eConfig>()
            .try_get::<String>("db.url")
            .unwrap(),
        "postgres://late-override"
    );
}

/// A late override must also be pinned: a provider's runtime watch may not
/// overwrite it (same guarantee the pre-`load_config` path gets).
#[tokio::test]
async fn late_override_config_value_pins_runtime_live_updates() {
    let app = AppBuilder::new()
        .override_config(R2eConfig::empty())
        .with_config_provider(WatchProvider {
            key: "db.url",
            value: "provider-update",
        })
        .load_config::<()>()
        .override_config_value("db.url", "test-override")
        .build_state()
        .await;

    let registry = app.state().get::<LiveConfigRegistry>();
    let sink = ConfigUpdateSink::new(registry.clone());
    assert!(!sink.set("db.url", "provider-update"));
    assert_eq!(registry.get::<String>("db.url").unwrap(), "test-override");
}

/// End-to-end: a producer's `#[live_config]` handle resolves the late override.
#[tokio::test]
async fn late_override_config_value_is_visible_to_producer_handle() {
    let mut config = R2eConfig::empty();
    config.set("db.url", ConfigValue::String("postgres://boot".to_string()));

    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .override_config_value("db.url", "postgres://late-override")
        .register::<ProduceDbUrl>()
        .build_state()
        .await;

    let handle = app.state().get::<LiveConfig<String>>();
    assert_eq!(handle.get().unwrap(), "postgres://late-override");
}

#[tokio::test]
async fn producer_live_config_param_gets_runtime_handle() {
    let mut config = R2eConfig::empty();
    config.set("db.url", ConfigValue::String("postgres://boot".to_string()));

    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .register::<ProduceDbUrl>()
        .build_state()
        .await;

    let handle = app.state().get::<LiveConfig<String>>();
    assert_eq!(handle.get().unwrap(), "postgres://boot");
}

/// A `#[live_config("key")]` param is declared in `config_keys()` with kind
/// `Live`: an absent key never fails startup validation (the handle's `get()`
/// returns a `Result`), and the key stays out of the producer's dev-reload
/// fingerprint (freshness arrives by push, not by rebuild).
#[test]
fn live_config_param_is_subscribed_not_copied() {
    let keys = <ProduceDbUrl as Producer>::config_keys();
    let entry = keys
        .iter()
        .find(|(key, _, _)| *key == "db.url")
        .expect("live_config key must appear in config_keys()");
    assert_eq!(entry.2, ConfigKeyKind::Live);
    assert!(
        !entry.2.is_required(),
        "live_config keys must not be presence-validated"
    );
    assert!(
        !entry.2.is_fingerprinted(),
        "live_config keys must not rebuild the producer when edited"
    );
}

/// The absent-at-boot case the `Live` kind exists for: a producer
/// with a `#[live_config]` param whose key is missing from the config still
/// builds, and the handle simply reports the missing key at `get()` time.
#[tokio::test]
async fn producer_live_config_param_tolerates_missing_key_at_boot() {
    let app = AppBuilder::new()
        .override_config(R2eConfig::empty())
        .load_config::<()>()
        .register::<ProduceDbUrl>()
        .build_state()
        .await;

    let handle = app.state().get::<LiveConfig<String>>();
    assert!(handle.get().is_err());

    // Once a provider pushes the value at runtime, the same handle sees it.
    let registry = app.state().get::<LiveConfigRegistry>();
    assert!(registry.set("db.url", "postgres://late"));
    assert_eq!(handle.get().unwrap(), "postgres://late");
}
