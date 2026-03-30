use std::any::{type_name, TypeId};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use r2e_core::beans::{AsyncBean, Bean, BeanContext, BeanError, BeanRegistry, PostConstruct, Producer};
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

// ── PostConstruct tests ────────────────────────────────────────────────

#[derive(Clone)]
struct InitTracker {
    initialized: Arc<AtomicBool>,
}

impl Bean for InitTracker {
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    fn build(_ctx: &BeanContext) -> Self {
        Self {
            initialized: Arc::new(AtomicBool::new(false)),
        }
    }
    fn after_register(registry: &mut BeanRegistry) {
        registry.register_post_construct::<Self>();
    }
}

impl PostConstruct for InitTracker {
    fn post_construct(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send + '_>,
    > {
        Box::pin(async move {
            self.initialized.store(true, Ordering::SeqCst);
            Ok(())
        })
    }
}

#[tokio::test]
async fn post_construct_is_called() {
    let mut reg = BeanRegistry::new();
    reg.register::<InitTracker>();
    let ctx = reg.resolve().await.unwrap();

    let tracker: InitTracker = ctx.get();
    assert!(tracker.initialized.load(Ordering::SeqCst));
}

#[derive(Clone)]
struct FailingBean;

impl Bean for FailingBean {
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    fn build(_ctx: &BeanContext) -> Self {
        Self
    }
    fn after_register(registry: &mut BeanRegistry) {
        registry.register_post_construct::<Self>();
    }
}

impl PostConstruct for FailingBean {
    fn post_construct(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send + '_>,
    > {
        Box::pin(async move { Err("init failed".into()) })
    }
}

#[tokio::test]
async fn post_construct_error_propagates() {
    let mut reg = BeanRegistry::new();
    reg.register::<FailingBean>();
    let err = reg.resolve().await.unwrap_err();
    match &err {
        BeanError::PostConstruct(msg) => {
            assert!(msg.contains("init failed"), "error: {msg}");
        }
        _ => panic!("expected PostConstruct error, got {:?}", err),
    }
}

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

#[tokio::test]
async fn optional_dep_none_when_absent() {
    let mut reg = BeanRegistry::new();
    reg.provide(Dep { value: 10 });
    reg.register::<OptionalConsumer>();
    let ctx = reg.resolve().await.unwrap();

    let consumer: OptionalConsumer = ctx.get();
    assert_eq!(consumer.dep.value, 10);
    assert!(consumer.optional.is_none());
}

#[tokio::test]
async fn optional_dep_some_when_provided() {
    let mut reg = BeanRegistry::new();
    reg.provide(Dep { value: 20 });
    reg.provide(ServiceA { dep: Dep { value: 20 } });
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

#[tokio::test]
async fn all_optional_deps_none() {
    let mut reg = BeanRegistry::new();
    reg.register::<AllOptional>();
    let ctx = reg.resolve().await.unwrap();

    let bean: AllOptional = ctx.get();
    assert!(bean.a.is_none());
    assert!(bean.b.is_none());
}

#[tokio::test]
async fn all_optional_deps_some_when_provided() {
    let dep = Dep { value: 5 };
    let svc_a = ServiceA { dep: dep.clone() };
    let svc_b = ServiceB { a: svc_a.clone(), dep: dep.clone() };

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

#[tokio::test]
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
