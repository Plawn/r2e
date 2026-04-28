//! Verifies `#[async_exec]` codegen on a `#[routes]` controller — the
//! marked method must return a `JobHandle<T>` whose result matches what
//! the original body would have produced inline.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use r2e_core::http::extract::FromRef;
use r2e_core::prelude::*;
use r2e_executor::{ExecutorConfig, JobHandle, PoolExecutor};

#[derive(Clone)]
struct Services {
    executor: PoolExecutor,
    counter: Arc<AtomicU32>,
}

impl FromRef<Services> for PoolExecutor {
    fn from_ref(s: &Services) -> Self { s.executor.clone() }
}
impl FromRef<Services> for Arc<AtomicU32> {
    fn from_ref(s: &Services) -> Self { s.counter.clone() }
}

#[derive(Controller, Clone)]
#[controller(state = Services)]
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
async fn async_exec_returns_job_handle() {
    let services = Services {
        executor: PoolExecutor::new(ExecutorConfig::default()),
        counter: Arc::new(AtomicU32::new(0)),
    };

    let worker = Worker::from_state(&services);
    let handle: JobHandle<u32> = worker.compute(21);
    let result = handle.await.expect("job succeeds");

    assert_eq!(result, 42);
    assert_eq!(services.counter.load(Ordering::SeqCst), 1);
}
