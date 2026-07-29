use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use r2e_core::{AppBuilder, BeanContext, ServiceComponent};
use tokio_util::sync::CancellationToken;

static STARTED: AtomicUsize = AtomicUsize::new(0);
static STOPPED: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone)]
struct ProbeService;

impl ServiceComponent for ProbeService {
    fn from_context(ctx: &BeanContext) -> Self {
        ctx.get::<ProbeService>()
    }

    async fn start(self, shutdown: CancellationToken) {
        STARTED.fetch_add(1, Ordering::SeqCst);
        shutdown.cancelled().await;
        STOPPED.fetch_add(1, Ordering::SeqCst);
    }
}

#[r2e_macros::producer(start)]
fn make_probe_service() -> ProbeService {
    ProbeService
}

#[tokio::test]
async fn producer_start_runs_output_as_tracked_service() {
    STARTED.store(0, Ordering::SeqCst);
    STOPPED.store(0, Ordering::SeqCst);

    let app = AppBuilder::new()
        .register::<MakeProbeService>()
        .build_state()
        .await;
    let prepared = app.prepare("127.0.0.1:0");
    let stop = prepared.stop_handle();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = tokio::spawn(async move {
        prepared
            .run_with_listener(listener)
            .await
            .map_err(|e| e.to_string())
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(STARTED.load(Ordering::SeqCst), 1);

    stop.stop();
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(STOPPED.load(Ordering::SeqCst), 1);
}
