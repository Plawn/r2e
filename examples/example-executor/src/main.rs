//! example-executor — managed `PoolExecutor` + `BackgroundService`.
//!
//! Demonstrates:
//! - `Executor` plugin → makes `PoolExecutor` injectable.
//! - `#[async_exec]` on a controller method → returns `JobHandle<T>`.
//! - `#[derive(BackgroundService)]` → tick worker that submits jobs to the
//!   pool until shutdown.
//!
//! Run with:
//! ```bash
//! cargo run -p example-executor
//! curl -X POST http://localhost:3000/reports/123
//! curl http://localhost:3000/metrics
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use r2e::prelude::*;
use r2e::r2e_executor::{Executor, JobHandle, PoolExecutor};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

#[derive(Clone, BeanState)]
pub struct Services {
    pub executor: PoolExecutor,
    pub counter: Arc<AtomicU64>,
    pub config: R2eConfig,
}

#[derive(Serialize)]
struct ReportSummary {
    id: u64,
    status: &'static str,
    bytes: usize,
}

#[derive(Serialize)]
struct ExecMetrics {
    running: u64,
    queued: u64,
    completed: u64,
    rejected: u64,
    background_ticks: u64,
}

#[derive(Controller, Clone)]
#[controller(path = "/", state = Services)]
pub struct ReportController {
    #[inject] executor: PoolExecutor,
    #[inject] counter: Arc<AtomicU64>,
}

#[routes]
impl ReportController {
    /// Synchronously fires a long job and returns immediately with a handle.
    #[post("/reports/:id")]
    async fn create(&self, Path(id): Path<u64>) -> Json<ReportSummary> {
        let _job: JobHandle<Vec<u8>> = self.generate_pdf(id);
        Json(ReportSummary { id, status: "queued", bytes: 0 })
    }

    /// Awaits the result inline — useful when the caller wants the bytes.
    #[get("/reports/:id")]
    async fn fetch(&self, Path(id): Path<u64>) -> Json<ReportSummary> {
        let bytes = self.generate_pdf(id).await.expect("job ok");
        Json(ReportSummary { id, status: "ready", bytes: bytes.len() })
    }

    #[get("/metrics")]
    async fn metrics(&self) -> Json<ExecMetrics> {
        let m = self.executor.metrics();
        Json(ExecMetrics {
            running: m.running,
            queued: m.queued,
            completed: m.completed,
            rejected: m.rejected,
            background_ticks: self.counter.load(Ordering::SeqCst),
        })
    }

    /// Body runs on the `PoolExecutor`; returns `JobHandle<Vec<u8>>`.
    #[async_exec]
    async fn generate_pdf(&self, id: u64) -> Vec<u8> {
        tokio::time::sleep(Duration::from_millis(150)).await;
        format!("PDF for report #{id}").into_bytes()
    }
}

#[derive(BackgroundService, Clone)]
#[service(state = Services)]
pub struct TickWorker {
    #[inject] executor: PoolExecutor,
    #[inject] counter: Arc<AtomicU64>,
}

impl TickWorker {
    async fn run(&self, shutdown: CancellationToken) {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = interval.tick() => {
                    let counter = self.counter.clone();
                    self.executor.submit_detached(async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                    });
                }
            }
        }
    }
}

#[r2e::main]
async fn main() {
    AppBuilder::new()
        .with_config(R2eConfig::empty())
        .plugin(Executor)
        .provide(Arc::new(AtomicU64::new(0)))
        .build_state::<Services, _, _>()
        .await
        .with(Health)
        .with(Cors::permissive())
        .spawn_service::<TickWorker>()
        .register_controller::<ReportController>()
        .serve("0.0.0.0:3000")
        .await
        .unwrap();
}
