//! `Option<T>` as a first-class bean type — its own `TypeId`, always-present slot.

use std::any::{type_name, TypeId};
use std::sync::Arc;

use r2e_core::beans::{Bean, BeanContext, BeanError, BeanRegistry, Producer};
use r2e_core::type_list::TNil;

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
    assert_eq!(
        consumer.client.unwrap().endpoint,
        "https://example.azure.com"
    );
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
