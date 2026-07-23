//! Default vs. alternative beans and the override rules.

use std::any::{type_name, TypeId};

use r2e_core::beans::{Bean, BeanContext, BeanError, BeanRegistry, Producer};
use r2e_core::type_list::TNil;

use crate::fixtures::Dep;

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
