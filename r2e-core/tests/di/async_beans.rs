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
