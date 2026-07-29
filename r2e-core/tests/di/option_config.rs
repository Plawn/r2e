//! `Option<T>` fed by `#[config]` params on beans and producers.

use r2e_core::beans::{Bean, BeanError, BeanRegistry, Producer};
use r2e_core::config::ConfigKeyKind;

// ── Option<T> `#[config]` params (optional config) ──────────────────────
//
// Regression for Tasker task #670: a `#[config("key")] param: Option<T>` must
// be OPTIONAL — an absent key resolves to `None` (not a `MissingConfigKeys`
// abort), a present value resolves to `Some(v)`, and a required (non-Option)
// key still fails when absent. Uses the exact real-world repro key
// `database.min-idle` (dashed) with type `Option<u32>`.

#[derive(Clone, Debug, PartialEq)]
struct DbSettings {
    min_idle: Option<u32>,
}

#[r2e_core::prelude::producer]
fn create_db_settings(#[config("database.min-idle")] min_idle: Option<u32>) -> DbSettings {
    DbSettings { min_idle }
}

#[r2e_core::test]
async fn producer_option_config_absent_resolves_none() {
    // Key not present anywhere: build succeeds and the value is `None`.
    let mut reg = BeanRegistry::new();
    reg.provide(r2e_core::config::R2eConfig::empty());
    reg.register_producer::<CreateDbSettings>();
    let ctx = reg.resolve().await.unwrap();

    let settings: DbSettings = ctx.get();
    assert_eq!(settings.min_idle, None);
}

#[r2e_core::test]
async fn producer_option_config_present_resolves_some() {
    let mut config = r2e_core::config::R2eConfig::empty();
    config.set(
        "database.min-idle",
        r2e_core::config::ConfigValue::Integer(5),
    );

    let mut reg = BeanRegistry::new();
    reg.provide(config);
    reg.register_producer::<CreateDbSettings>();
    let ctx = reg.resolve().await.unwrap();

    let settings: DbSettings = ctx.get();
    assert_eq!(settings.min_idle, Some(5));
}

#[derive(Clone, Debug, PartialEq)]
struct RequiredSettings {
    #[allow(dead_code)]
    max_idle: u32,
}

#[r2e_core::prelude::producer]
fn create_required_settings(#[config("database.max-idle")] max_idle: u32) -> RequiredSettings {
    RequiredSettings { max_idle }
}

#[r2e_core::test]
async fn producer_required_config_absent_still_fails_validation() {
    // A non-Option `#[config]` key remains required — absence is a
    // `MissingConfigKeys` abort (existing behavior preserved).
    let mut reg = BeanRegistry::new();
    reg.provide(r2e_core::config::R2eConfig::empty());
    reg.register_producer::<CreateRequiredSettings>();
    let err = reg.resolve().await.unwrap_err();
    assert!(
        matches!(err, BeanError::MissingConfigKeys(_)),
        "required config key must still fail validation when absent: {err:?}"
    );
}

#[derive(Clone, Debug, PartialEq)]
struct OptConfigBean {
    label: Option<String>,
}

#[r2e_core::prelude::bean]
impl OptConfigBean {
    fn new(#[config("app.label")] label: Option<String>) -> Self {
        Self { label }
    }
}

#[r2e_core::test]
async fn bean_option_config_absent_resolves_none() {
    let mut reg = BeanRegistry::new();
    reg.provide(r2e_core::config::R2eConfig::empty());
    reg.register::<OptConfigBean>();
    let ctx = reg.resolve().await.unwrap();

    let bean: OptConfigBean = ctx.get();
    assert_eq!(bean.label, None);
}

#[r2e_core::test]
async fn bean_option_config_present_resolves_some() {
    let mut config = r2e_core::config::R2eConfig::empty();
    config.set(
        "app.label",
        r2e_core::config::ConfigValue::String("prod".into()),
    );

    let mut reg = BeanRegistry::new();
    reg.provide(config);
    reg.register::<OptConfigBean>();
    let ctx = reg.resolve().await.unwrap();

    let bean: OptConfigBean = ctx.get();
    assert_eq!(bean.label.as_deref(), Some("prod"));
}

// Fix 1 (dev-reload fingerprint): EVERY *copied* `#[config]` key — optional
// included — must appear in `config_keys()` so an edit to an optional value
// under `r2e dev` rebuilds the bean. The `ConfigKeyKind` (3rd tuple element)
// is `Optional` for `Option<T>` keys (skipped by presence validation) and
// `Required` otherwise — both are fingerprinted.
#[test]
fn producer_option_config_key_is_fingerprinted_but_not_required() {
    let keys = <CreateDbSettings as Producer>::config_keys();
    assert_eq!(
        keys,
        vec![(
            "database.min-idle",
            "Option < u32 >",
            ConfigKeyKind::Optional
        )],
        "optional config key must be present as Optional: {keys:?}"
    );
    assert!(!keys[0].2.is_required());
    assert!(
        keys[0].2.is_fingerprinted(),
        "an optional copied key is still fingerprinted"
    );
}

#[test]
fn producer_required_config_key_is_required() {
    let keys = <CreateRequiredSettings as Producer>::config_keys();
    assert_eq!(
        keys,
        vec![("database.max-idle", "u32", ConfigKeyKind::Required)]
    );
    assert!(keys[0].2.is_required());
    assert!(keys[0].2.is_fingerprinted());
}

#[test]
fn bean_option_config_key_is_fingerprinted_but_not_required() {
    let keys = <OptConfigBean as Bean>::config_keys();
    assert_eq!(
        keys,
        vec![("app.label", "Option < String >", ConfigKeyKind::Optional)]
    );
    assert!(keys[0].2.is_fingerprinted());
}
