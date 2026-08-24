//! Verifies `#[derive(BackgroundService)]` codegen — the generated impl
//! must build the struct from the resolved bean graph and forward `start`
//! to `run`.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use r2e_core::prelude::BackgroundService;
use r2e_core::rt::{self, CancelToken};
use r2e_core::ServiceComponent;
use r2e_executor::{ExecutorConfig, PoolExecutor};

#[derive(BackgroundService, Clone)]
struct Worker {
    #[inject]
    executor: PoolExecutor,
    #[inject]
    counter: Arc<AtomicU32>,
}

impl Worker {
    async fn run(&self, shutdown: CancelToken) {
        loop {
            let c = self.counter.clone();
            self.executor.submit_detached(async move {
                c.fetch_add(1, Ordering::SeqCst);
            });
            rt::select! {
                _ = shutdown.cancelled() => break,
                _ = rt::sleep(Duration::from_millis(10)) => {}
            }
        }
    }
}

#[tokio::test]
async fn derive_background_service_runs_until_cancelled() {
    let counter = Arc::new(AtomicU32::new(0));

    let mut registry = r2e_core::BeanRegistry::new();
    registry.provide(PoolExecutor::new(ExecutorConfig::default()));
    registry.provide(counter.clone());
    let ctx = registry.resolve().await.unwrap();

    let worker = Worker::from_context(&ctx);
    let token = CancelToken::new();
    let task = rt::spawn(worker.start(token.clone()));

    rt::sleep(Duration::from_millis(80)).await;
    token.cancel();
    task.await.expect("worker exits cleanly on cancel");

    assert!(
        counter.load(Ordering::SeqCst) > 0,
        "worker should have submitted at least one job"
    );
}
