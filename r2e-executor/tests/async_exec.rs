//! Verifies `#[async_exec]` codegen on a `#[routes]` controller — the
//! marked method must return a `JobHandle<T>` whose result matches what
//! the original body would have produced inline.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use r2e_core::prelude::*;
use r2e_executor::{ExecutorConfig, JobHandle, PoolExecutor, RejectedError};

#[controller]
#[derive(Clone)]
struct Worker {
    #[inject] executor: PoolExecutor,
    #[inject] counter: Arc<AtomicU32>,
}

#[routes]
impl Worker {
    #[async_exec]
    async fn compute(&self, base: u32) -> u32 {
        self.counter.fetch_add(1, Ordering::SeqCst);
        base * 2
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
