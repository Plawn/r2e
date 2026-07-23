//! Resolving the bean graph: simple wiring, missing deps, duplicates, cycles.

use std::any::{type_name, TypeId};

use r2e_core::beans::{Bean, BeanContext, BeanError, BeanRegistry};
use r2e_core::type_list::TNil;

use crate::fixtures::{Dep, ServiceA, ServiceB};

#[r2e_core::test]
async fn resolve_simple_graph() {
    let mut reg = BeanRegistry::new();
    reg.provide(Dep { value: 42 });
    reg.register::<ServiceA>();
    reg.register::<ServiceB>();
    let ctx = reg.resolve().await.unwrap();

    let b: ServiceB = ctx.get();
    assert_eq!(b.dep.value, 42);
    assert_eq!(b.a.dep.value, 42);
}

#[r2e_core::test]
async fn missing_dependency() {
    let mut reg = BeanRegistry::new();
    reg.register::<ServiceA>();
    let err = reg.resolve().await.unwrap_err();
    match &err {
        BeanError::MissingDependency { dependency, .. } => {
            assert!(
                dependency.contains("Dep"),
                "error should name the missing type: {}",
                err
            );
        }
        _ => panic!("expected MissingDependency, got {:?}", err),
    }
}

#[r2e_core::test]
async fn duplicate_bean_registered_twice() {
    let mut reg = BeanRegistry::new();
    reg.provide(Dep { value: 1 });
    reg.register::<ServiceA>();
    reg.register::<ServiceA>();
    let err = reg.resolve().await.unwrap_err();
    assert!(matches!(err, BeanError::DuplicateBean { .. }));
}

#[r2e_core::test]
async fn duplicate_provided_and_bean() {
    let mut reg = BeanRegistry::new();
    reg.provide(Dep { value: 1 });
    reg.provide(ServiceA {
        dep: Dep { value: 2 },
    });
    reg.register::<ServiceA>();
    let err = reg.resolve().await.unwrap_err();
    assert!(matches!(err, BeanError::DuplicateBean { .. }));
}

#[derive(Clone)]
struct CycleA;
#[derive(Clone)]
struct CycleB;

impl Bean for CycleA {
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![(TypeId::of::<CycleB>(), type_name::<CycleB>())]
    }
    fn build(ctx: &BeanContext) -> Self {
        let _ = ctx.get::<CycleB>();
        Self
    }
}
impl Bean for CycleB {
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![(TypeId::of::<CycleA>(), type_name::<CycleA>())]
    }
    fn build(ctx: &BeanContext) -> Self {
        let _ = ctx.get::<CycleA>();
        Self
    }
}

#[r2e_core::test]
async fn cyclic_dependency() {
    let mut reg = BeanRegistry::new();
    reg.register::<CycleA>();
    reg.register::<CycleB>();
    let err = reg.resolve().await.unwrap_err();
    let BeanError::CyclicDependency { cycle } = err else {
        panic!("expected CyclicDependency, got: {err}");
    };
    // The reported path is a closed loop: A -> B -> A (3 entries, ends where
    // it starts).
    assert_eq!(cycle.len(), 3);
    assert_eq!(cycle.first(), cycle.last());
    assert!(cycle.iter().any(|n| n.contains("CycleA")));
    assert!(cycle.iter().any(|n| n.contains("CycleB")));
}

/// Depends on the A↔B cycle but is not part of it — must not appear in the
/// reported cycle path.
#[derive(Clone)]
struct CycleDependent;

impl Bean for CycleDependent {
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![(TypeId::of::<CycleA>(), type_name::<CycleA>())]
    }
    fn build(ctx: &BeanContext) -> Self {
        let _ = ctx.get::<CycleA>();
        Self
    }
}

#[r2e_core::test]
async fn cyclic_dependency_reports_only_the_cycle() {
    let mut reg = BeanRegistry::new();
    reg.register::<CycleDependent>();
    reg.register::<CycleA>();
    reg.register::<CycleB>();
    let err = reg.resolve().await.unwrap_err();
    let BeanError::CyclicDependency { cycle } = err else {
        panic!("expected CyclicDependency, got: {err}");
    };
    // CycleDependent is stuck behind the cycle but is not on it.
    assert_eq!(cycle.len(), 3);
    assert_eq!(cycle.first(), cycle.last());
    assert!(!cycle.iter().any(|n| n.contains("CycleDependent")));
}

#[r2e_core::test]
async fn provided_only() {
    let mut reg = BeanRegistry::new();
    reg.provide(Dep { value: 7 });
    let ctx = reg.resolve().await.unwrap();
    let d: Dep = ctx.get();
    assert_eq!(d.value, 7);
}

#[r2e_core::test]
async fn try_get_none() {
    let reg = BeanRegistry::new();
    let ctx = reg.resolve().await.unwrap();
    assert!(ctx.try_get::<Dep>().is_none());
}

#[r2e_core::test]
async fn empty_registry() {
    let reg = BeanRegistry::new();
    let ctx = reg.resolve().await.unwrap();
    assert!(ctx.try_get::<i32>().is_none());
}
