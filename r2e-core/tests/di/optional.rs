//! `Option<T>` dependencies that resolve to `None` when absent.

use std::any::{type_name, TypeId};
use std::sync::atomic::Ordering;

use r2e_core::beans::{Bean, BeanContext, BeanRegistry};
use r2e_core::type_list::TNil;

use crate::lifecycle::InitTracker;

use crate::fixtures::{Dep, ServiceA, ServiceB};

// ── Optional dependency tests ──────────────────────────────────────────

/// Bean with an optional dependency via Option<T> — resolved to None when absent.
#[derive(Clone)]
struct OptionalConsumer {
    dep: Dep,
    optional: Option<ServiceA>,
}

impl Bean for OptionalConsumer {
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        // Only hard dep is Dep — ServiceA is optional
        vec![(TypeId::of::<Dep>(), type_name::<Dep>())]
    }
    fn build(ctx: &BeanContext) -> Self {
        Self {
            dep: ctx.get::<Dep>(),
            optional: ctx.try_get::<ServiceA>(),
        }
    }
}

#[r2e_core::test]
async fn optional_dep_none_when_absent() {
    let mut reg = BeanRegistry::new();
    reg.provide(Dep { value: 10 });
    reg.register::<OptionalConsumer>();
    let ctx = reg.resolve().await.unwrap();

    let consumer: OptionalConsumer = ctx.get();
    assert_eq!(consumer.dep.value, 10);
    assert!(consumer.optional.is_none());
}

#[r2e_core::test]
async fn optional_dep_some_when_provided() {
    let mut reg = BeanRegistry::new();
    reg.provide(Dep { value: 20 });
    reg.provide(ServiceA {
        dep: Dep { value: 20 },
    });
    reg.register::<OptionalConsumer>();
    let ctx = reg.resolve().await.unwrap();

    let consumer: OptionalConsumer = ctx.get();
    assert_eq!(consumer.dep.value, 20);
    assert!(consumer.optional.is_some());
    assert_eq!(consumer.optional.unwrap().dep.value, 20);
}

/// Bean with only optional dependencies — no hard deps at all.
#[derive(Clone)]
struct AllOptional {
    a: Option<ServiceA>,
    b: Option<ServiceB>,
}

impl Bean for AllOptional {
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![] // no hard deps
    }
    fn build(ctx: &BeanContext) -> Self {
        Self {
            a: ctx.try_get::<ServiceA>(),
            b: ctx.try_get::<ServiceB>(),
        }
    }
}

#[r2e_core::test]
async fn all_optional_deps_none() {
    let mut reg = BeanRegistry::new();
    reg.register::<AllOptional>();
    let ctx = reg.resolve().await.unwrap();

    let bean: AllOptional = ctx.get();
    assert!(bean.a.is_none());
    assert!(bean.b.is_none());
}

#[r2e_core::test]
async fn all_optional_deps_some_when_provided() {
    let dep = Dep { value: 5 };
    let svc_a = ServiceA { dep: dep.clone() };
    let svc_b = ServiceB {
        a: svc_a.clone(),
        dep: dep.clone(),
    };

    let mut reg = BeanRegistry::new();
    reg.provide(dep);
    reg.provide(svc_a);
    reg.provide(svc_b);
    reg.register::<AllOptional>();
    let ctx = reg.resolve().await.unwrap();

    let bean: AllOptional = ctx.get();
    assert!(bean.a.is_some());
    assert!(bean.b.is_some());
}

// ── PostConstruct with dependencies (continued) ──────────────────────

#[r2e_core::test]
async fn post_construct_with_dependencies() {
    // InitTracker depends on nothing, ServiceA depends on Dep.
    // Verify post_construct runs after the full graph is resolved.
    let mut reg = BeanRegistry::new();
    reg.provide(Dep { value: 42 });
    reg.register::<ServiceA>();
    reg.register::<InitTracker>();
    let ctx = reg.resolve().await.unwrap();

    let tracker: InitTracker = ctx.get();
    assert!(tracker.initialized.load(Ordering::SeqCst));

    let a: ServiceA = ctx.get();
    assert_eq!(a.dep.value, 42);
}
