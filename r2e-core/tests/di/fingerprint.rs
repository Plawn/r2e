//! Graph fingerprint used by dev-reload to decide what to rebuild.

use r2e_core::beans::BeanRegistry;

use crate::fixtures::{Dep, ServiceA, ServiceB};
use crate::lazy_bean::LazyCfgOptional;

// ── Graph fingerprint (dev-reload) ──────────────────────────────────────────

#[test]
fn compute_fingerprint_stable_for_same_graph() {
    fn registry() -> BeanRegistry {
        let mut reg = BeanRegistry::new();
        reg.provide(Dep { value: 1 });
        reg.register::<ServiceA>();
        reg.register::<ServiceB>();
        reg
    }
    let (fp1, per_bean1) = registry().compute_fingerprint().unwrap();
    let (fp2, per_bean2) = registry().compute_fingerprint().unwrap();
    assert_eq!(fp1, fp2);
    assert_eq!(per_bean1, per_bean2);
    assert_eq!(per_bean1.len(), 2);
}

#[test]
fn compute_fingerprint_changes_when_graph_differs() {
    let mut small = BeanRegistry::new();
    small.provide(Dep { value: 1 });
    small.register::<ServiceA>();
    let (fp_small, _) = small.compute_fingerprint().unwrap();

    let mut big = BeanRegistry::new();
    big.provide(Dep { value: 1 });
    big.register::<ServiceA>();
    big.register::<ServiceB>();
    let (fp_big, _) = big.compute_fingerprint().unwrap();

    assert_ne!(fp_small, fp_big);
}

#[test]
fn compute_fingerprint_changes_on_config_edit() {
    fn registry(value: &str) -> BeanRegistry {
        let mut config = r2e_core::config::R2eConfig::empty();
        config.set(
            "app.greeting",
            r2e_core::config::ConfigValue::String(value.into()),
        );
        let mut reg = BeanRegistry::new();
        reg.provide(config);
        reg.register::<LazyCfgOptional>();
        reg
    }
    // Even a key no bean requires participates in the graph fingerprint.
    let (fp_a, _) = registry("hello").compute_fingerprint().unwrap();
    let (fp_b, _) = registry("bonjour").compute_fingerprint().unwrap();
    assert_ne!(fp_a, fp_b);
}

// ── #[config_section] — a prefix, not an exact key ──────────────────────────

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct DbProps {
    url: String,
    #[config(default = 5)]
    pool_size: i64,
}

#[derive(Clone, r2e_core::prelude::Bean)]
struct HoldsSection {
    #[config_section(prefix = "db")]
    #[allow(dead_code)]
    db: DbProps,
}

/// A `#[config_section]` field declares its **prefix** in `config_keys()`, with
/// kind `Section`: fingerprinted (so a section edit rebuilds the bean) but never
/// presence-validated (`ConfigProperties::from_config` is the validator).
#[test]
fn config_section_declares_its_prefix_as_a_section_key() {
    use r2e_core::beans::Bean;
    use r2e_core::config::ConfigKeyKind;

    let keys = <HoldsSection as Bean>::config_keys();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].0, "db", "the entry's key is the section prefix");
    assert_eq!(keys[0].2, ConfigKeyKind::Section);
    assert!(keys[0].2.is_prefix());
    assert!(keys[0].2.is_fingerprinted());
    assert!(
        !keys[0].2.is_required(),
        "sections validate at construction, not through presence checks"
    );
}

/// The per-bean fingerprint must move when *any* key under the declared prefix
/// moves — including one the exact-key hashing could never have named — and stay
/// put for edits outside it.
#[test]
fn per_bean_fingerprint_tracks_every_key_under_the_section_prefix() {
    fn per_bean(pool: i64, unrelated: &str) -> u64 {
        use r2e_core::config::ConfigValue;
        let mut config = r2e_core::config::R2eConfig::empty();
        config.set("db.url", ConfigValue::String("postgres://x".into()));
        config.set("db.pool_size", ConfigValue::Integer(pool));
        config.set("app.unrelated", ConfigValue::String(unrelated.into()));
        let mut reg = BeanRegistry::new();
        reg.provide(config);
        reg.register::<HoldsSection>();
        let (_, per_bean) = reg.compute_fingerprint().unwrap();
        per_bean
            .iter()
            .find(|(tid, _, _)| *tid == std::any::TypeId::of::<HoldsSection>())
            .expect("the section-holding bean must be fingerprinted")
            .2
    }

    assert_ne!(
        per_bean(10, "x"),
        per_bean(20, "x"),
        "editing a key inside the section must move the bean fingerprint"
    );
    assert_eq!(
        per_bean(10, "x"),
        per_bean(10, "y"),
        "editing a key outside the section must leave it alone"
    );
}

/// `prefix_fingerprint` covers the prefix key itself, every descendant, and
/// nothing else — and reacts to a key being added or removed, not only edited.
#[test]
fn prefix_fingerprint_covers_the_subtree_only() {
    use r2e_core::config::{ConfigValue, R2eConfig};

    let mut base = R2eConfig::empty();
    base.set("db.url", ConfigValue::String("a".into()));
    base.set("other.url", ConfigValue::String("z".into()));

    let mut edited_inside = base.clone();
    edited_inside.set("db.url", ConfigValue::String("b".into()));
    assert_ne!(
        base.prefix_fingerprint("db"),
        edited_inside.prefix_fingerprint("db")
    );

    let mut edited_outside = base.clone();
    edited_outside.set("other.url", ConfigValue::String("y".into()));
    assert_eq!(
        base.prefix_fingerprint("db"),
        edited_outside.prefix_fingerprint("db")
    );

    let mut grown = base.clone();
    grown.set("db.pool_size", ConfigValue::Integer(5));
    assert_ne!(
        base.prefix_fingerprint("db"),
        grown.prefix_fingerprint("db"),
        "a key appearing inside the section must move the digest"
    );

    // Prefixes are not interchangeable even when their subtrees hash alike.
    assert_ne!(
        base.prefix_fingerprint("db"),
        base.prefix_fingerprint("nope")
    );
}
