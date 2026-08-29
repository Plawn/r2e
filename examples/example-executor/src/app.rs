// example-executor — managed `PoolExecutor` + `BackgroundService`.
//
// Demonstrates:
// - `Executor` plugin → makes `PoolExecutor` injectable.
// - `#[async_exec]` on a controller method → returns `Result<JobHandle<T>, RejectedError>`.
// - `#[derive(BackgroundService)]` → tick worker that submits jobs to the
//   pool until shutdown.
//
// Run with:
//   cargo run -p example-executor
//   curl -X POST http://localhost:3000/reports/123
//   curl http://localhost:3000/metrics
//
// The canonical source lives in `app.rs` (included by `lib.rs` and by
// `app_main!` in the binary tip crate).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use r2e::prelude::*;
use r2e::r2e_executor::{Executor, PoolExecutor};
use r2e::rt::{self, CancelToken};
use serde::Serialize;

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

#[controller(path = "/")]
#[derive(Clone)]
pub struct ReportController {
    #[inject] executor: PoolExecutor,
    #[inject] counter: Arc<AtomicU64>,
}

#[routes]
impl ReportController {
    /// Synchronously fires a long job and returns immediately with a handle.
    #[post("/reports/:id")]
    async fn create(&self, Path(id): Path<u64>) -> Json<ReportSummary> {
        let _job = self.generate_pdf(id).expect("executor running");
        Json(ReportSummary { id, status: "queued", bytes: 0 })
    }

    /// Awaits the result inline — useful when the caller wants the bytes.
    #[get("/reports/:id")]
    async fn fetch(&self, Path(id): Path<u64>) -> Json<ReportSummary> {
        let bytes = self.generate_pdf(id).expect("executor running").await.expect("job ok");
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

    /// Body runs on the `PoolExecutor`; returns `Result<JobHandle<Vec<u8>>, RejectedError>`.
    #[async_exec]
    async fn generate_pdf(&self, id: u64) -> Vec<u8> {
        rt::sleep(Duration::from_millis(150)).await;
        format!("PDF for report #{id}").into_bytes()
    }
}

#[derive(BackgroundService, Clone)]
pub struct TickWorker {
    #[inject] executor: PoolExecutor,
    #[inject] counter: Arc<AtomicU64>,
}

impl TickWorker {
    async fn run(&self, shutdown: CancelToken) {
        let mut interval = rt::interval(Duration::from_secs(1));
        loop {
            rt::select! {
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

/// The canonical application blueprint.
pub struct ExecutorApp;

impl App for ExecutorApp {
    /// The tick counter is shared between the controller and the background
    /// worker; in dev mode it survives hot-patches.
    type Env = Arc<AtomicU64>;

    async fn setup() -> Result<Arc<AtomicU64>, BootError> {
        Ok({
        Arc::new(AtomicU64::new(0))
    })
    }

    async fn build(b: AppBuilder, counter: Arc<AtomicU64>) -> Result<impl BootableApp, BootError> {
        Ok({
        // No config file: an empty in-memory config keeps `load_config` from
        // reading disk (and `serve_auto` then defaults to 0.0.0.0:3000).
        b.override_config(R2eConfig::empty())
            .load_config::<()>()
            .plugin(Executor)
            .provide(counter)
            .plugin(Health)
            .plugin(Cors::permissive())
            .build_state()
            .await
            .spawn_service::<TickWorker>()
            .register_controller::<ReportController>()
    })
    }
}
