//! `#[live_config("key")]` **field** injection on beans.
//!
//! The field form is the declarative twin of the `#[producer]` parameter form
//! covered in `tests/config/provider.rs`: a `LiveConfig<T>` handle resolved once
//! at construction from the `LiveConfigRegistry` bean, with the key declared
//! `ConfigKeyKind::Live` in `config_keys()` — never presence-validated and
//! never fingerprinted (its freshness comes from the registry push).

use r2e_core::beans::{Bean, BeanRegistry};
use r2e_core::config::{
    ConfigKeyKind, ConfigUpdateSink, ConfigValue, LiveConfig, LiveConfigRegistry, R2eConfig,
};
use r2e_core::{AppBuilder, BeanAccess};

#[derive(Clone, r2e_core::prelude::Bean)]
struct LiveUrlService {
    #[live_config("db.url")]
    url: LiveConfig<String>,
    #[config("app.name")]
    name: String,
}

/// A bean with a live handle and no plain `#[config]` at all — its dependency
/// list must still carry `LiveConfigRegistry`.
#[derive(Clone, r2e_core::prelude::Bean)]
struct LiveOnlyService {
    #[live_config("feature.flag")]
    flag: LiveConfig<bool>,
}

fn config_with_url() -> R2eConfig {
    let mut config = R2eConfig::empty();
    config.set("db.url", ConfigValue::String("postgres://boot".into()));
    config.set("app.name", ConfigValue::String("demo".into()));
    config.set("feature.flag", ConfigValue::Bool(false));
    config
}

#[r2e_core::test]
async fn bean_live_config_field_reads_boot_value() {
    let app = AppBuilder::new()
        .override_config(config_with_url())
        .load_config::<()>()
        .register::<LiveUrlService>()
        .register::<LiveOnlyService>()
        .build_state()
        .await;

    let service = app.state().get::<LiveUrlService>();
    assert_eq!(service.url.get().unwrap(), "postgres://boot");
    // Plain `#[config]` on the same struct keeps its boot snapshot semantics.
    assert_eq!(service.name, "demo");
    assert!(!app.state().get::<LiveOnlyService>().flag.get().unwrap());
}

#[r2e_core::test]
async fn bean_live_config_field_sees_runtime_updates() {
    let app = AppBuilder::new()
        .override_config(config_with_url())
        .load_config::<()>()
        .register::<LiveUrlService>()
        .build_state()
        .await;
    let service = app.state().get::<LiveUrlService>();

    // A provider watcher pushing a new value through the sink is visible to the
    // handle the bean captured at construction — no rebuild involved.
    let sink = ConfigUpdateSink::new(app.state().get::<LiveConfigRegistry>());
    assert!(sink.set("db.url", "postgres://rotated"));
    assert_eq!(service.url.get().unwrap(), "postgres://rotated");
    assert_eq!(service.url.snapshot().version(), 2);
}

/// The absent-at-boot case `required = false` exists for: the bean builds with
/// the key missing and the handle reports it at `get()` time only.
#[r2e_core::test]
async fn bean_live_config_field_tolerates_missing_key_at_boot() {
    let mut config = R2eConfig::empty();
    config.set("app.name", ConfigValue::String("demo".into()));

    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .register::<LiveUrlService>()
        .build_state()
        .await;

    let service = app.state().get::<LiveUrlService>();
    assert!(service.url.get().is_err());

    let registry = app.state().get::<LiveConfigRegistry>();
    assert!(registry.set("db.url", "postgres://late"));
    assert_eq!(service.url.get().unwrap(), "postgres://late");
}

/// The key is declared `Live`: reported for introspection, never presence-
/// validated, and — unlike a copied key — never fingerprinted, so editing it
/// under `r2e dev` pushes instead of rebuilding. Mirrors
/// `live_config_param_is_subscribed_not_copied`.
#[test]
fn bean_live_config_field_is_subscribed_not_copied() {
    let keys = <LiveUrlService as Bean>::config_keys();

    let live = keys
        .iter()
        .find(|(key, _, _)| *key == "db.url")
        .expect("live_config key must appear in config_keys()");
    assert_eq!(live.2, ConfigKeyKind::Live);
    assert!(
        !live.2.is_required(),
        "live_config keys must not be presence-validated"
    );
    assert!(
        !live.2.is_fingerprinted(),
        "live_config keys must not rebuild the bean when edited"
    );
    assert_eq!(live.1, "LiveConfig < String >");

    let plain = keys
        .iter()
        .find(|(key, _, _)| *key == "app.name")
        .expect("plain config key must still appear");
    assert_eq!(
        plain.2,
        ConfigKeyKind::Required,
        "a non-Option #[config] key stays required"
    );
    assert!(plain.2.is_fingerprinted());
}

/// The bean must declare `LiveConfigRegistry` as a dependency: without
/// `load_config` (which provides it) resolution reports the standard missing
/// dependency instead of panicking inside `build`.
#[r2e_core::test]
async fn bean_live_config_field_declares_registry_dependency() {
    assert!(<LiveOnlyService as Bean>::dependencies()
        .iter()
        .any(|(id, _)| *id == std::any::TypeId::of::<LiveConfigRegistry>()));

    let mut reg = BeanRegistry::new();
    reg.register::<LiveOnlyService>();
    let err = reg.resolve().await.unwrap_err();
    assert!(
        err.to_string().contains("LiveConfigRegistry"),
        "missing registry must be reported: {err}"
    );
}
