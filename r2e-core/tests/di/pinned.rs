//! Pinned overrides — the test-harness primitive behind `override_bean`.

use std::any::TypeId;

use r2e_core::beans::{Bean, BeanContext, BeanRegistry};
use r2e_core::type_list::TNil;

use crate::fixtures::{Dep, ServiceA};

// ── Pinned overrides (test-harness primitive) ───────────────────────────

#[r2e_core::test]
async fn pin_provide_wins_over_later_provide() {
    let mut reg = BeanRegistry::new();
    reg.pin_provide(Dep { value: 1 });
    reg.provide(Dep { value: 2 });
    let ctx = reg.resolve().await.unwrap();

    let dep: Dep = ctx.get();
    assert_eq!(
        dep.value, 1,
        "pinned instance must win over a later provide"
    );
}

#[r2e_core::test]
async fn pin_provide_wins_over_later_register() {
    #[derive(Clone)]
    struct Marked {
        origin: &'static str,
    }

    impl Bean for Marked {
        type Deps = TNil;
        fn dependencies() -> Vec<(TypeId, &'static str)> {
            vec![]
        }
        fn build(_ctx: &BeanContext) -> Self {
            Self { origin: "real" }
        }
    }

    let mut reg = BeanRegistry::new();
    reg.pin_provide(Marked { origin: "pinned" });
    reg.register::<Marked>();
    let ctx = reg.resolve().await.unwrap();

    let m: Marked = ctx.get();
    assert_eq!(
        m.origin, "pinned",
        "pinned instance must win over a later register"
    );
}

#[r2e_core::test]
async fn pinned_bean_feeds_dependents() {
    // A bean depending on the pinned type must receive the pinned instance.
    let mut reg = BeanRegistry::new();
    reg.pin_provide(Dep { value: 99 });
    reg.provide(Dep { value: 0 });
    reg.register::<ServiceA>();
    let ctx = reg.resolve().await.unwrap();

    let a: ServiceA = ctx.get();
    assert_eq!(a.dep.value, 99);
}

#[r2e_core::test]
async fn pin_provide_without_later_registration_is_harmless() {
    let mut reg = BeanRegistry::new();
    reg.pin_provide(Dep { value: 7 });
    let ctx = reg.resolve().await.unwrap();

    let dep: Dep = ctx.get();
    assert_eq!(dep.value, 7);
}
