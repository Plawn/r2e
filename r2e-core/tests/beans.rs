use std::any::{type_name, TypeId};

use r2e_core::beans::{AsyncBean, Bean, BeanContext, BeanError, BeanRegistry, Producer};
use r2e_core::type_list::TNil;

#[derive(Clone)]
struct Dep {
    value: i32,
}

#[derive(Clone)]
struct ServiceA {
    dep: Dep,
}

impl Bean for ServiceA {
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![(TypeId::of::<Dep>(), type_name::<Dep>())]
    }
    fn build(ctx: &BeanContext) -> Self {
        Self {
            dep: ctx.get::<Dep>(),
        }
    }
}

#[derive(Clone)]
struct ServiceB {
    a: ServiceA,
    dep: Dep,
}

impl Bean for ServiceB {
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![
            (TypeId::of::<ServiceA>(), type_name::<ServiceA>()),
            (TypeId::of::<Dep>(), type_name::<Dep>()),
        ]
    }
    fn build(ctx: &BeanContext) -> Self {
        Self {
            a: ctx.get::<ServiceA>(),
            dep: ctx.get::<Dep>(),
        }
    }
}

#[tokio::test]
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

#[tokio::test]
async fn missing_dependency() {
    let mut reg = BeanRegistry::new();
    reg.register::<ServiceA>();
    let err = reg.resolve().await.unwrap_err();
    match &err {
        BeanError::MissingDependency { dependency, .. } => {
            assert!(dependency.contains("Dep"), "error should name the missing type: {}", err);
        }
        _ => panic!("expected MissingDependency, got {:?}", err),
    }
}

#[tokio::test]
async fn duplicate_bean_registered_twice() {
    let mut reg = BeanRegistry::new();
    reg.provide(Dep { value: 1 });
    reg.register::<ServiceA>();
    reg.register::<ServiceA>();
    let err = reg.resolve().await.unwrap_err();
    assert!(matches!(err, BeanError::DuplicateBean { .. }));
}

#[tokio::test]
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

#[tokio::test]
async fn cyclic_dependency() {
    let mut reg = BeanRegistry::new();
    reg.register::<CycleA>();
    reg.register::<CycleB>();
    let err = reg.resolve().await.unwrap_err();
    assert!(matches!(err, BeanError::CyclicDependency { .. }));
}

#[tokio::test]
async fn provided_only() {
    let mut reg = BeanRegistry::new();
    reg.provide(Dep { value: 7 });
    let ctx = reg.resolve().await.unwrap();
    let d: Dep = ctx.get();
    assert_eq!(d.value, 7);
}

#[tokio::test]
async fn try_get_none() {
    let reg = BeanRegistry::new();
    let ctx = reg.resolve().await.unwrap();
    assert!(ctx.try_get::<Dep>().is_none());
}

#[tokio::test]
async fn empty_registry() {
    let reg = BeanRegistry::new();
    let ctx = reg.resolve().await.unwrap();
    assert!(ctx.try_get::<i32>().is_none());
}

// ── Async bean tests ──────────────────────────────────────────────────

#[derive(Clone)]
struct AsyncService {
    dep: Dep,
}

impl AsyncBean for AsyncService {
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![(TypeId::of::<Dep>(), type_name::<Dep>())]
    }
    async fn build(ctx: &BeanContext) -> Self {
        // Simulate async init
        tokio::task::yield_now().await;
        Self {
            dep: ctx.get::<Dep>(),
        }
    }
}

#[tokio::test]
async fn async_bean_resolution() {
    let mut reg = BeanRegistry::new();
    reg.provide(Dep { value: 99 });
    reg.register_async::<AsyncService>();
    let ctx = reg.resolve().await.unwrap();

    let svc: AsyncService = ctx.get();
    assert_eq!(svc.dep.value, 99);
}

#[tokio::test]
async fn mixed_sync_async_graph() {
    let mut reg = BeanRegistry::new();
    reg.provide(Dep { value: 10 });
    reg.register::<ServiceA>();          // sync: depends on Dep
    reg.register_async::<AsyncService>(); // async: depends on Dep
    let ctx = reg.resolve().await.unwrap();

    let a: ServiceA = ctx.get();
    let svc: AsyncService = ctx.get();
    assert_eq!(a.dep.value, 10);
    assert_eq!(svc.dep.value, 10);
}

// ── Producer tests ────────────────────────────────────────────────────

#[derive(Clone)]
struct DbPool {
    url: String,
}

struct CreateDbPool;

impl Producer for CreateDbPool {
    type Output = DbPool;
    type Deps = TNil;

    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }

    async fn produce(_ctx: &BeanContext) -> DbPool {
        // Simulate async pool creation
        tokio::task::yield_now().await;
        DbPool {
            url: "sqlite::memory:".to_string(),
        }
    }
}

#[tokio::test]
async fn producer_resolution() {
    let mut reg = BeanRegistry::new();
    reg.register_producer::<CreateDbPool>();
    let ctx = reg.resolve().await.unwrap();

    let pool: DbPool = ctx.get();
    assert_eq!(pool.url, "sqlite::memory:");
}

#[tokio::test]
async fn producer_as_dependency() {
    // Producer creates DbPool, then a sync bean depends on it.
    #[derive(Clone)]
    struct RepoService {
        pool: DbPool,
    }

    impl Bean for RepoService {
        type Deps = TNil;
        fn dependencies() -> Vec<(TypeId, &'static str)> {
            vec![(TypeId::of::<DbPool>(), type_name::<DbPool>())]
        }
        fn build(ctx: &BeanContext) -> Self {
            Self {
                pool: ctx.get::<DbPool>(),
            }
        }
    }

    let mut reg = BeanRegistry::new();
    reg.register_producer::<CreateDbPool>();
    reg.register::<RepoService>();
    let ctx = reg.resolve().await.unwrap();

    let repo: RepoService = ctx.get();
    assert_eq!(repo.pool.url, "sqlite::memory:");
}
