//! `LiveConfigRegistry` internals: lazy slot creation (a slot is materialized
//! and boot-seeded on first access, not one per config entry at load) and the
//! typed read cache on `LiveConfig<T>` (one `FromConfigValue` conversion per
//! snapshot version, shared by every clone of the handle).

use std::collections::HashSet;

use r2e_core::config::{ConfigUpdateSink, ConfigValue, LiveConfig, LiveConfigRegistry, R2eConfig};

fn boot_config(pairs: &[(&str, &str)]) -> R2eConfig {
    let mut config = R2eConfig::empty();
    for (key, value) in pairs {
        config.set(key, ConfigValue::String((*value).to_string()));
    }
    config
}

/// The cache must never shadow a newer snapshot: a sink write bumps the
/// version, so the next `get()` reconverts instead of replaying the boot value.
#[test]
fn cached_handle_serves_runtime_updates() {
    let registry = LiveConfigRegistry::from_config(
        &boot_config(&[("db.url", "postgres://boot")]),
        HashSet::new(),
    );
    let handle: LiveConfig<String> = registry.live_config("db.url");
    assert_eq!(handle.get().unwrap(), "postgres://boot");

    let sink = ConfigUpdateSink::new(registry.clone());
    assert!(sink.set("db.url", "postgres://runtime"));
    assert_eq!(handle.get().unwrap(), "postgres://runtime");
}

/// Repeated reads at the same version are stable (they come off the cache), and
/// a write still moves the value.
#[test]
fn repeated_reads_are_stable_until_a_write() {
    let registry =
        LiveConfigRegistry::from_config(&boot_config(&[("feature.limit", "10")]), HashSet::new());
    let handle: LiveConfig<u16> = registry.live_config("feature.limit");

    assert_eq!(handle.get().unwrap(), 10);
    assert_eq!(handle.get().unwrap(), 10);
    assert_eq!(handle.snapshot().version(), 1);

    registry.set("feature.limit", 42i64);
    assert_eq!(handle.get().unwrap(), 42);
    assert_eq!(handle.get().unwrap(), 42);
    assert_eq!(handle.snapshot().version(), 2);
}

/// A conversion failure is never cached: the handle keeps erroring while the
/// bad value stands, and recovers as soon as a convertible value is published.
#[test]
fn conversion_errors_are_not_cached_and_recover() {
    let registry = LiveConfigRegistry::new();
    registry.set("feature.limit", "not-a-number");

    let handle: LiveConfig<u16> = registry.live_config("feature.limit");
    assert!(handle.get().is_err());
    // Still an error on a second read — nothing poisoned, nothing cached.
    assert!(handle.get().is_err());

    registry.set("feature.limit", 7i64);
    assert_eq!(handle.get().unwrap(), 7);
}

/// A good value must never be replayed for a newer, unconvertible snapshot.
#[test]
fn a_stale_cache_entry_is_not_served_for_a_newer_snapshot() {
    let registry =
        LiveConfigRegistry::from_config(&boot_config(&[("feature.limit", "10")]), HashSet::new());
    let handle: LiveConfig<u16> = registry.live_config("feature.limit");
    assert_eq!(handle.get().unwrap(), 10);

    registry.set("feature.limit", "not-a-number");
    assert!(handle.get().is_err());
}

/// A key absent at boot yields an empty slot: `NotFound` until a runtime write.
#[test]
fn missing_key_errors_until_a_runtime_write() {
    let registry = LiveConfigRegistry::from_config(&R2eConfig::empty(), HashSet::new());
    let handle: LiveConfig<String> = registry.live_config("db.url");

    assert!(handle.get().is_err());
    assert_eq!(handle.snapshot().version(), 0);
    assert!(handle.snapshot().updated_at().is_none());

    assert!(registry.set("db.url", "postgres://late"));
    assert_eq!(handle.get().unwrap(), "postgres://late");
}

/// The lazy-seed path: a slot created long after boot still reports the boot
/// value, at version 1, stamped with the boot timestamp.
#[test]
fn slots_created_after_boot_are_seeded_from_the_boot_config() {
    let config = boot_config(&[("db.url", "postgres://boot"), ("other.key", "unused")]);
    let registry = LiveConfigRegistry::from_config(&config, HashSet::new());

    // Nothing touched this key yet — the slot only exists from here on.
    let handle: LiveConfig<String> = registry.live_config("db.url");
    assert_eq!(handle.get().unwrap(), "postgres://boot");

    let snapshot = handle.snapshot();
    assert_eq!(snapshot.version(), 1);
    assert!(snapshot.updated_at().is_some());
    assert!(snapshot.last_error().is_none());
    assert!(!snapshot.is_stale());

    // The registry read path seeds the same way, through a different entry.
    assert_eq!(registry.get::<String>("other.key").unwrap(), "unused");
}

/// Pinning must survive lazy creation: the pinned set is captured at
/// `from_config`, but the slot it protects appears only on first access.
#[test]
fn pinning_blocks_sink_writes_on_lazily_created_slots() {
    let config = boot_config(&[("db.url", "postgres://override")]);
    let registry = LiveConfigRegistry::from_config(&config, HashSet::from(["db.url".to_string()]));

    let sink = ConfigUpdateSink::new(registry.clone());
    // First touch of the key is the provider write itself — it must be refused
    // and must not create a slot carrying the provider value.
    assert!(!sink.set("db.url", "postgres://provider"));

    let handle: LiveConfig<String> = registry.live_config("db.url");
    assert_eq!(handle.get().unwrap(), "postgres://override");
    assert!(!sink.set("db.url", "postgres://provider-again"));
    assert_eq!(handle.get().unwrap(), "postgres://override");
}

/// Handle clones share one slot and one cache — an update is visible through
/// every clone, whichever one reads first.
#[test]
fn handle_clones_share_slot_and_cache() {
    let registry = LiveConfigRegistry::from_config(
        &boot_config(&[("db.url", "postgres://boot")]),
        HashSet::new(),
    );
    let handle: LiveConfig<String> = registry.live_config("db.url");
    let clone = handle.clone();

    assert_eq!(handle.get().unwrap(), "postgres://boot");
    assert_eq!(clone.get().unwrap(), "postgres://boot");
    assert_eq!(clone.key(), "db.url");

    registry.set("db.url", "postgres://runtime");
    // The clone reads first: both must see the new value.
    assert_eq!(clone.get().unwrap(), "postgres://runtime");
    assert_eq!(handle.get().unwrap(), "postgres://runtime");
}

/// `subscribe()` uses the handle's stored slot — it must observe writes that
/// happen after the receiver was created.
#[tokio::test]
async fn subscribe_observes_writes_on_the_handle_slot() {
    let registry = LiveConfigRegistry::from_config(
        &boot_config(&[("db.url", "postgres://boot")]),
        HashSet::new(),
    );
    let handle: LiveConfig<String> = registry.live_config("db.url");
    let mut rx = handle.subscribe();

    registry.set("db.url", "postgres://runtime");
    rx.changed().await.unwrap();
    assert_eq!(rx.borrow_and_update().unwrap(), "postgres://runtime");
}

// ── dead-key diagnostic ──────────────────────────────────────────────────
//
// Live keys are deliberately never presence-validated, so a typo'd
// `#[live_config("db.ulr")]` cannot fail startup. `live_config()` emits a
// `tracing::warn!` instead, gated on `is_dead_key`: no value anywhere AND no
// registered `ConfigProvider` that could ever supply one. The tests below pin
// the gate down (asserting the log line itself would need a subscriber capture
// harness the workspace does not have; the precondition is the whole logic).

/// A key that is present at boot is never dead, however it was written.
#[test]
fn a_key_with_a_boot_value_is_not_dead() {
    let registry = LiveConfigRegistry::from_config(
        &boot_config(&[("db.url", "postgres://boot")]),
        HashSet::new(),
    );
    assert!(!registry.is_dead_key("db.url"));
}

/// The typo case: absent at boot, and no provider registered to fill it in.
#[test]
fn a_key_absent_at_boot_without_providers_is_dead() {
    let registry =
        LiveConfigRegistry::from_config(&boot_config(&[("db.url", "x")]), HashSet::new());
    assert!(
        registry.is_dead_key("db.ulr"),
        "a misspelt key with no writer in sight is exactly what the warning is for"
    );
    // Hand-built registries (tests, direct use) default to "no providers", so
    // they warn too — nobody registered a writer, so nobody will.
    assert!(LiveConfigRegistry::new().is_dead_key("anything"));
}

/// A value pushed at runtime before the handle exists proves the key is alive,
/// even though it was absent at boot.
#[test]
fn a_key_written_at_runtime_is_not_dead() {
    let registry = LiveConfigRegistry::new();
    assert!(registry.is_dead_key("feature.flag"));
    registry.set("feature.flag", "on");
    assert!(
        !registry.is_dead_key("feature.flag"),
        "a value already in the slot means something IS writing this key"
    );
}

/// With a `ConfigProvider` registered, an absent key may legitimately arrive
/// later through the watch task — so `load_config` marks the registry and the
/// diagnostic stays quiet.
#[tokio::test]
async fn a_registered_config_provider_silences_the_diagnostic() {
    use r2e_core::config::{ConfigError, ConfigProvider, ConfigProviderContext};
    use r2e_core::type_list::BeanAccess;
    use r2e_core::AppBuilder;

    struct SilentProvider;
    impl ConfigProvider for SilentProvider {
        fn load(
            &self,
            _config: &mut R2eConfig,
            _ctx: ConfigProviderContext<'_>,
        ) -> Result<(), ConfigError> {
            Ok(())
        }
    }

    let without = AppBuilder::new()
        .override_config(R2eConfig::empty())
        .load_config::<()>()
        .build_state()
        .await;
    assert!(without.state().get::<LiveConfigRegistry>().is_dead_key("db.url"));

    let with = AppBuilder::new()
        .override_config(R2eConfig::empty())
        .with_config_provider(SilentProvider)
        .load_config::<()>()
        .build_state()
        .await;
    assert!(
        !with.state().get::<LiveConfigRegistry>().is_dead_key("db.url"),
        "a provider may fill the key in at runtime, so absence is not evidence of a typo"
    );
}
