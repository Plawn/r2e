//! Verifies `#[async_exec]` codegen on a `#[routes]` controller and on a
//! `#[bean]` impl — the marked method must return a `JobHandle<T>` whose
//! result matches what the original body would have produced inline.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use r2e_core::prelude::*;
use r2e_executor::{ExecutorConfig, JobHandle, PoolExecutor, RejectedError};

#[controller]
#[derive(Clone)]
struct Worker {
    #[inject]
    executor: PoolExecutor,
    #[inject]
    counter: Arc<AtomicU32>,
}

#[routes]
impl Worker {
    #[async_exec]
    async fn compute(&self, base: u32) -> u32 {
        self.counter.fetch_add(1, Ordering::SeqCst);
        base * 2
    }

    /// A parameter attribute must reach the inner fn, the wrapper signature AND
    /// the forwarding call: a `#[cfg]` is NOT pre-evaluated by rustc on a
    /// *parameter*, so gating only the signatures leaves the disabled build
    /// passing an argument that has no binding — and one argument too many
    /// (task #985). `any()` is never true, so `disabled` must vanish everywhere,
    /// including from the call; naming a type that does not exist is what proves
    /// it did.
    #[async_exec]
    async fn compute_gated(
        &self,
        base: u32,
        #[cfg(any())] disabled: ThisTypeDoesNotExist,
        #[cfg(all())] enabled: u32,
    ) -> u32 {
        self.counter.fetch_add(1, Ordering::SeqCst);
        base * 2 + enabled
    }
}

#[derive(Clone)]
struct ReportService {
    io_pool: PoolExecutor,
    counter: Arc<AtomicU32>,
}

#[bean]
impl ReportService {
    fn new(io_pool: PoolExecutor, counter: Arc<AtomicU32>) -> Self {
        Self { io_pool, counter }
    }

    #[async_exec(executor = "io_pool")]
    async fn generate(&self, base: u32) -> u32 {
        self.counter.fetch_add(1, Ordering::SeqCst);
        base * 3
    }

    /// Same cfg-gated-parameter rule on the `#[bean]` host (task #985).
    #[async_exec(executor = "io_pool")]
    async fn generate_gated(
        &self,
        base: u32,
        #[cfg(any())] disabled: ThisTypeDoesNotExist,
        #[cfg(all())] enabled: u32,
    ) -> u32 {
        self.counter.fetch_add(1, Ordering::SeqCst);
        base * 3 + enabled
    }
}

#[tokio::test]
async fn async_exec_returns_join_handle() {
    let counter = Arc::new(AtomicU32::new(0));

    let mut registry = r2e_core::BeanRegistry::new();
    registry.provide(PoolExecutor::new(ExecutorConfig::default()));
    registry.provide(counter.clone());
    let ctx = registry.resolve().await.unwrap();

    let worker = <Worker as r2e_core::ContextConstruct>::from_context(&ctx);
    let handle: Result<JobHandle<u32>, RejectedError> = worker.compute(21);
    let result = handle.expect("submit ok").await.expect("job succeeds");

    assert_eq!(result, 42);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn bean_async_exec_returns_join_handle() {
    let counter = Arc::new(AtomicU32::new(0));

    let mut registry = r2e_core::BeanRegistry::new();
    registry.provide(PoolExecutor::new(ExecutorConfig::default()));
    registry.provide(counter.clone());
    registry.register::<ReportService>();
    let ctx = registry.resolve().await.unwrap();

    let service = ctx.get::<ReportService>();
    let handle: Result<JobHandle<u32>, RejectedError> = service.generate(14);
    let result = handle.expect("submit ok").await.expect("job succeeds");

    assert_eq!(result, 42);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn async_exec_cfg_gated_params_forward_correctly() {
    let counter = Arc::new(AtomicU32::new(0));

    let mut registry = r2e_core::BeanRegistry::new();
    registry.provide(PoolExecutor::new(ExecutorConfig::default()));
    registry.provide(counter.clone());
    registry.register::<ReportService>();
    let ctx = registry.resolve().await.unwrap();

    // Only the cfg-enabled parameter survives, on both hosts: the wrappers take
    // exactly two arguments and the disabled one is gone from the call too.
    let worker = <Worker as r2e_core::ContextConstruct>::from_context(&ctx);
    let handle: Result<JobHandle<u32>, RejectedError> = worker.compute_gated(10, 5);
    assert_eq!(handle.expect("submit ok").await.expect("job succeeds"), 25);

    let service = ctx.get::<ReportService>();
    let handle: Result<JobHandle<u32>, RejectedError> = service.generate_gated(10, 5);
    assert_eq!(handle.expect("submit ok").await.expect("job succeeds"), 35);

    assert_eq!(counter.load(Ordering::SeqCst), 2);
}
