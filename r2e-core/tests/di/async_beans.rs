//! `AsyncBean` and mixed sync/async graphs.

use std::any::{type_name, TypeId};

use r2e_core::beans::{AsyncBean, BeanContext, BeanRegistry};
use r2e_core::type_list::TNil;

use crate::fixtures::{Dep, ServiceA};

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
    reg.register::<ServiceA>(); // sync: depends on Dep
    reg.register_async::<AsyncService>(); // async: depends on Dep
    let ctx = reg.resolve().await.unwrap();

    let a: ServiceA = ctx.get();
    let svc: AsyncService = ctx.get();
    assert_eq!(a.dep.value, 10);
    assert_eq!(svc.dep.value, 10);
}

// ── Constructor futures need not be `Send` ────────────────────────────
//
// `AsyncBean::build` / `Producer::produce` deliberately return a future
// WITHOUT a `+ Send` bound: the graph is resolved in place on the boot thread
// and never spawned. The bound used to be there and, being checked for all
// lifetimes, rejected ordinary bodies (sqlx `Acquire`/`Executor` reborrows —
// rust-lang/rust#100013). These beans hold a `!Send` value across an await, so
// they only compile while that bound stays off.

#[derive(Clone)]
struct NotSendCtorBean {
    value: i32,
}

impl AsyncBean for NotSendCtorBean {
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![(TypeId::of::<Dep>(), type_name::<Dep>())]
    }
    async fn build(ctx: &BeanContext) -> Self {
        let local = std::rc::Rc::new(ctx.get::<Dep>().value);
        r2e_core::rt::yield_now().await;
        Self { value: *local + 1 }
    }
}

#[derive(Clone)]
struct NotSendProduced(i32);

struct NotSendProducer;

impl r2e_core::beans::Producer for NotSendProducer {
    type Output = NotSendProduced;
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![(TypeId::of::<Dep>(), type_name::<Dep>())]
    }
    async fn produce(ctx: &BeanContext) -> Self::Output {
        let local = std::rc::Rc::new(ctx.get::<Dep>().value);
        r2e_core::rt::yield_now().await;
        NotSendProduced(*local + 2)
    }
}

#[r2e_core::test]
async fn async_constructors_may_be_non_send() {
    let mut reg = BeanRegistry::new();
    reg.provide(Dep { value: 7 });
    reg.register_async::<NotSendCtorBean>();
    reg.register_producer::<NotSendProducer>();
    let ctx = reg.resolve().await.unwrap();

    assert_eq!(ctx.get::<NotSendCtorBean>().value, 8);
    assert_eq!(ctx.get::<NotSendProduced>().0, 9);
}
