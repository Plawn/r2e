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
            assert!(dependency.contains("Dep"), "error should name the missing type: {}", err);
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
    assert!(matches!(err, BeanError::CyclicDependency { .. }));
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

#[r2e_core::test]
async fn async_bean_resolution() {
    let mut reg = BeanRegistry::new();
    reg.provide(Dep { value: 99 });
    reg.register_async::<AsyncService>();
    let ctx = reg.resolve().await.unwrap();

    let svc: AsyncService = ctx.get();
    assert_eq!(svc.dep.value, 99);
}

#[r2e_core::test]
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

#[r2e_core::test]
async fn producer_resolution() {
    let mut reg = BeanRegistry::new();
    reg.register_producer::<CreateDbPool>();
    let ctx = reg.resolve().await.unwrap();

    let pool: DbPool = ctx.get();
    assert_eq!(pool.url, "sqlite::memory:");
}

#[r2e_core::test]
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

#[r2e_core::test]
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

#[r2e_core::test]
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

// ── Default / Alternative bean tests ─────────────────────────────────

#[derive(Clone)]
struct InMemoryCache {
    kind: &'static str,
}

impl Bean for InMemoryCache {
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    fn build(_ctx: &BeanContext) -> Self {
        Self { kind: "in-memory" }
    }
}

/// Shared output type for the default/alternative producer pattern.
#[derive(Clone)]
struct CacheImpl {
    kind: &'static str,
}

// A "default" version that produces CacheImpl
struct DefaultCacheProducer;
impl Producer for DefaultCacheProducer {
    type Output = CacheImpl;
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    async fn produce(_ctx: &BeanContext) -> CacheImpl {
        CacheImpl { kind: "in-memory" }
    }
}

// An "alternative" version that also produces CacheImpl
struct RedisCacheProducer;
impl Producer for RedisCacheProducer {
    type Output = CacheImpl;
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    async fn produce(_ctx: &BeanContext) -> CacheImpl {
        CacheImpl { kind: "redis" }
    }
}

#[r2e_core::test]
async fn default_bean_present_when_no_alternative() {
    let mut reg = BeanRegistry::new();
    reg.register_default::<InMemoryCache>();
    let ctx = reg.resolve().await.unwrap();

    let cache: InMemoryCache = ctx.get();
    assert_eq!(cache.kind, "in-memory");
}

#[r2e_core::test]
async fn alternative_replaces_default_when_condition_true() {
    let mut reg = BeanRegistry::new();
    reg.register_producer_default::<DefaultCacheProducer>();
    // Alternative replaces default (same TypeId: CacheImpl)
    reg.register_producer::<RedisCacheProducer>();
    let ctx = reg.resolve().await.unwrap();

    let cache: CacheImpl = ctx.get();
    assert_eq!(cache.kind, "redis");
}

#[r2e_core::test]
async fn default_stays_when_alternative_not_registered() {
    let mut reg = BeanRegistry::new();
    reg.register_producer_default::<DefaultCacheProducer>();
    // No alternative registered — default stays
    let ctx = reg.resolve().await.unwrap();

    let cache: CacheImpl = ctx.get();
    assert_eq!(cache.kind, "in-memory");
}

#[r2e_core::test]
async fn default_bean_with_dependencies() {
    // Default bean that has dependencies — should still resolve correctly.
    #[derive(Clone)]
    struct DefaultService {
        dep: Dep,
        kind: &'static str,
    }

    impl Bean for DefaultService {
        type Deps = TNil;
        fn dependencies() -> Vec<(TypeId, &'static str)> {
            vec![(TypeId::of::<Dep>(), type_name::<Dep>())]
        }
        fn build(ctx: &BeanContext) -> Self {
            Self {
                dep: ctx.get::<Dep>(),
                kind: "default",
            }
        }
    }

    let mut reg = BeanRegistry::new();
    reg.provide(Dep { value: 100 });
    reg.register_default::<DefaultService>();
    let ctx = reg.resolve().await.unwrap();

    let svc: DefaultService = ctx.get();
    assert_eq!(svc.dep.value, 100);
    assert_eq!(svc.kind, "default");
}

#[r2e_core::test]
async fn non_overridable_duplicate_still_errors() {
    // Two non-overridable registrations of the same type should still
    // produce a DuplicateBean error (no allow_overrides).
    let mut reg = BeanRegistry::new();
    reg.register::<InMemoryCache>();
    reg.register::<InMemoryCache>();
    let err = reg.resolve().await.unwrap_err();
    assert!(matches!(err, BeanError::DuplicateBean { .. }));
}

// ── Option<T> as a first-class bean type (issue plawn/data#39) ──────────
//
// In the first-class model, `Option<T>` is registered under its own
// `TypeId` — distinct from `T`. A producer returning `Option<T>` declares
// `type Output = Option<T>` and the bean context stores `Option<T>` verbatim.
//
// Consumers inject `Option<T>` as a **hard** dependency — the graph
// guarantees the slot exists (even if the inner value is `None`). This
// replaces the earlier "soft dependency" machinery: the compile-time
// type-list and the runtime topo sort both see `Option<T>` as a regular
// required type.

#[derive(Clone, Debug, PartialEq)]
struct LlmClient {
    endpoint: String,
}

/// Manual `Producer` impl whose output type is `Option<Arc<LlmClient>>`
/// and which returns `Some(...)`. The bean context receives the whole
/// `Option<Arc<LlmClient>>`, keyed on `TypeId::of::<Option<Arc<LlmClient>>>()`.
struct CreateLlmClientPresent;
impl Producer for CreateLlmClientPresent {
    type Output = Option<Arc<LlmClient>>;
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    async fn produce(_ctx: &BeanContext) -> Option<Arc<LlmClient>> {
        Some(Arc::new(LlmClient {
            endpoint: "https://example.azure.com".into(),
        }))
    }
}

/// Same shape, but returns `None`. The context still contains an entry
/// under `TypeId::of::<Option<Arc<LlmClient>>>()` — the value is `None`.
struct CreateLlmClientAbsent;
impl Producer for CreateLlmClientAbsent {
    type Output = Option<Arc<LlmClient>>;
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    async fn produce(_ctx: &BeanContext) -> Option<Arc<LlmClient>> {
        None
    }
}

#[r2e_core::test]
async fn producer_option_some_registers_option_type() {
    // Direct regression for plawn/data#39: a producer with
    // `type Output = Option<Arc<LlmClient>>` registers the whole Option,
    // and consumers look up `Option<Arc<LlmClient>>` as a hard dependency.
    let mut reg = BeanRegistry::new();
    reg.register_producer::<CreateLlmClientPresent>();
    let ctx = reg.resolve().await.unwrap();

    // The bean is keyed on `Option<Arc<LlmClient>>` — not on the inner type.
    let slot: Option<Arc<LlmClient>> = ctx.get();
    assert!(slot.is_some());
    assert_eq!(slot.unwrap().endpoint, "https://example.azure.com");

    assert!(ctx.try_get::<Option<Arc<LlmClient>>>().is_some());
    assert!(ctx.try_get::<Arc<LlmClient>>().is_none());
}

#[r2e_core::test]
async fn producer_option_none_still_registers_slot() {
    // Returning `None` still registers an entry — the value is `None`.
    let mut reg = BeanRegistry::new();
    reg.register_producer::<CreateLlmClientAbsent>();
    let ctx = reg.resolve().await.unwrap();

    let slot: Option<Arc<LlmClient>> = ctx.get();
    assert!(slot.is_none());

    // The inner type is not in the context.
    assert!(ctx.try_get::<Arc<LlmClient>>().is_none());
}

/// Consumer with a hard dependency on `Option<Arc<LlmClient>>`. The slot
/// is always present in the context — the consumer inspects the inner
/// `Option` to decide how to behave.
#[derive(Clone)]
struct LlmConsumer {
    client: Option<Arc<LlmClient>>,
}

impl Bean for LlmConsumer {
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        // Hard dep on the Option slot — not on the inner type.
        vec![(
            TypeId::of::<Option<Arc<LlmClient>>>(),
            type_name::<Option<Arc<LlmClient>>>(),
        )]
    }
    fn build(ctx: &BeanContext) -> Self {
        Self {
            client: ctx.get::<Option<Arc<LlmClient>>>(),
        }
    }
}

#[r2e_core::test]
async fn option_consumer_sees_some_when_producer_returns_some() {
    let mut reg = BeanRegistry::new();
    reg.register_producer::<CreateLlmClientPresent>();
    reg.register::<LlmConsumer>();
    let ctx = reg.resolve().await.unwrap();

    let consumer: LlmConsumer = ctx.get();
    assert!(consumer.client.is_some());
    assert_eq!(consumer.client.unwrap().endpoint, "https://example.azure.com");
}

#[r2e_core::test]
async fn option_consumer_sees_none_when_producer_returns_none() {
    let mut reg = BeanRegistry::new();
    reg.register_producer::<CreateLlmClientAbsent>();
    reg.register::<LlmConsumer>();
    let ctx = reg.resolve().await.unwrap();

    let consumer: LlmConsumer = ctx.get();
    assert!(consumer.client.is_none());
}

#[r2e_core::test]
async fn option_consumer_missing_dep_when_producer_not_registered() {
    // With the first-class model, consumers hard-depend on `Option<T>` —
    // if no producer registers the slot, resolution fails with a
    // `MissingDependency` error (same as any other unregistered hard dep).
    let mut reg = BeanRegistry::new();
    reg.register::<LlmConsumer>();
    let err = reg.resolve().await.unwrap_err();
    assert!(
        matches!(err, BeanError::MissingDependency { .. }),
        "expected MissingDependency, got {:?}",
        err
    );
}

// ── Macro-driven Option<T> producer + consumer ──────────────────────────
//
// Exercises the `#[producer]` and `#[bean]` macros with `Option<T>`
// return/param types. The macros emit `type Output = Option<T>` verbatim
// and `Option<T>` params become hard deps on `Option<T>`.

#[derive(Clone, Debug, PartialEq)]
struct CacheClient {
    backend: &'static str,
}

#[r2e_core::prelude::producer]
fn create_cache_present() -> Option<Arc<CacheClient>> {
    Some(Arc::new(CacheClient { backend: "redis" }))
}

#[r2e_core::prelude::producer]
fn create_cache_absent() -> Option<Arc<CacheClient>> {
    None
}

#[r2e_core::test]
async fn macro_producer_option_some_registers_option_slot() {
    let mut reg = BeanRegistry::new();
    reg.register_producer::<CreateCachePresent>();
    let ctx = reg.resolve().await.unwrap();

    let slot: Option<Arc<CacheClient>> = ctx.get();
    assert_eq!(slot.unwrap().backend, "redis");
    // Inner type is NOT in the context — only the Option slot.
    assert!(ctx.try_get::<Arc<CacheClient>>().is_none());
}

#[r2e_core::test]
async fn macro_producer_option_none_registers_none_slot() {
    let mut reg = BeanRegistry::new();
    reg.register_producer::<CreateCacheAbsent>();
    let ctx = reg.resolve().await.unwrap();

    let slot: Option<Arc<CacheClient>> = ctx.get();
    assert!(slot.is_none());
}

#[derive(Clone)]
struct CacheConsumer {
    cache: Option<Arc<CacheClient>>,
}

#[r2e_core::prelude::bean]
impl CacheConsumer {
    fn new(cache: Option<Arc<CacheClient>>) -> Self {
        Self { cache }
    }
}

#[r2e_core::test]
async fn macro_bean_option_consumer_sees_some_after_producer() {
    // The #[bean] macro emits a hard dep on `Option<Arc<CacheClient>>`.
    // The topological sort schedules the consumer after the producer.
    let mut reg = BeanRegistry::new();
    reg.register::<CacheConsumer>();
    reg.register_producer::<CreateCachePresent>();
    let ctx = reg.resolve().await.unwrap();

    let consumer: CacheConsumer = ctx.get();
    assert!(consumer.cache.is_some());
    assert_eq!(consumer.cache.unwrap().backend, "redis");
}

#[r2e_core::test]
async fn macro_bean_option_consumer_sees_none_when_producer_returns_none() {
    let mut reg = BeanRegistry::new();
    reg.register::<CacheConsumer>();
    reg.register_producer::<CreateCacheAbsent>();
    let ctx = reg.resolve().await.unwrap();

    let consumer: CacheConsumer = ctx.get();
    assert!(consumer.cache.is_none());
}

#[r2e_core::test]
async fn macro_bean_option_consumer_missing_when_no_producer() {
    // No producer → the `Option<Arc<CacheClient>>` slot isn't registered,
    // so the consumer fails with MissingDependency.
    let mut reg = BeanRegistry::new();
    reg.register::<CacheConsumer>();
    let err = reg.resolve().await.unwrap_err();
    assert!(matches!(err, BeanError::MissingDependency { .. }));
}
