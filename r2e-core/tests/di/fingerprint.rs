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
