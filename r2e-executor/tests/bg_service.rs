//! Verifies `#[derive(BackgroundService)]` codegen — the generated impl
//! must build the struct from app state and forward `start` to `run`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use r2e_core::http::extract::FromRef;
use r2e_core::ServiceComponent;
use r2e_core::prelude::BackgroundService;
use r2e_executor::{ExecutorConfig, PoolExecutor};
use tokio_util::sync::CancellationToken;

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

#[derive(BackgroundService, Clone)]
#[service(state = Services)]
struct Worker {
    #[inject] executor: PoolExecutor,
    #[inject] counter: Arc<AtomicU32>,
}

impl Worker {
    async fn run(&self, shutdown: CancellationToken) {
        loop {
            let c = self.counter.clone();
            self.executor.submit_detached(async move {
                c.fetch_add(1, Ordering::SeqCst);
            });
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_millis(10)) => {}
            }
        }
    }
}

#[tokio::test]
async fn derive_background_service_runs_until_cancelled() {
    let services = Services {
        executor: PoolExecutor::new(ExecutorConfig::default()),
        counter: Arc::new(AtomicU32::new(0)),
    };

    let worker = Worker::from_state(&services);
    let token = CancellationToken::new();
    let task = tokio::spawn(worker.start(token.clone()));

    tokio::time::sleep(Duration::from_millis(80)).await;
    token.cancel();
    task.await.expect("worker exits cleanly on cancel");

    assert!(services.counter.load(Ordering::SeqCst) > 0, "worker should have submitted at least one job");
}
