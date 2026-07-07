//! Integration test: scheduled tasks run on the control plane while the HTTP
//! server is sharded (`server.workers`).
//!
//! Mirrors `r2e-core/tests/sharded.rs`'s integration test: a free concrete
//! port, a readiness loop, graceful shutdown via SIGINT, and a 10s bounded
//! join. Gated to the platforms that support `SO_REUSEPORT`.

#![cfg(all(
    unix,
    not(any(target_os = "solaris", target_os = "illumos", target_os = "cygwin"))
))]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use r2e_core::builder::{ScheduledTaskMarker, TaskRegistryHandle};
use r2e_core::config::R2eConfig;
use r2e_core::{AppBuilder, BeanContext, BeanState, TCons, TNil};
use r2e_scheduler::{ScheduleConfig, Scheduler, ScheduledTask, ScheduledTaskDef};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

// ── Minimal test state (requires the scheduler-provided CancellationToken) ───

#[derive(Clone)]
struct TestState {
    #[allow(dead_code)]
    cancel: CancellationToken,
}

impl BeanState for TestState {
    type Requires = TCons<CancellationToken, TNil>;
    fn from_context(ctx: &BeanContext) -> Self {
        Self {
            cancel: ctx.get::<CancellationToken>(),
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    port
}

async fn request_ping(addr: &str) -> Result<(), String> {
    let mut stream = TcpStream::connect(addr)
        .await
        .map_err(|e| format!("connect: {e}"))?;
    stream
        .write_all(b"GET /ping HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .map_err(|e| format!("write: {e}"))?;
    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
        .await
        .map_err(|e| format!("read: {e}"))?;
    let text = String::from_utf8_lossy(&buf);
    if !text.starts_with("HTTP/1.1 200") {
        return Err(format!("unexpected status line: {:?}", text.lines().next()));
    }
    Ok(())
}

fn counting_task(
    name: &str,
    schedule: ScheduleConfig,
    counter: Arc<AtomicUsize>,
) -> Box<dyn std::any::Any + Send> {
    let def = ScheduledTaskDef {
        name: name.to_string(),
        schedule,
        state: counter,
        task: Box::new(|c: Arc<AtomicUsize>| {
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        }),
    };
    let trait_obj: Box<dyn ScheduledTask> = Box::new(def);
    Box::new(trait_obj)
}

// ── Integration test ───────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduled_task_ticks_while_sharded_and_shuts_down_clean() {
    use r2e_core::http::routing::get;

    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    let yaml = format!("server:\n  workers: 2\n  port: {port}\n");
    let config = R2eConfig::from_yaml_str(&yaml).unwrap();

    let counter = Arc::new(AtomicUsize::new(0));

    // Build the sharded app with the scheduler plugin installed.
    let app = AppBuilder::new()
        .with_config(config)
        .plugin(Scheduler)
        .build_typed_state::<TestState, _>()
        .await;

    // Register a fast interval task through the scheduler registry; the serve
    // hook starts it on the control plane when `run()` begins serving.
    let registry = app
        .get_plugin_data::<TaskRegistryHandle>()
        .expect("scheduler registry should exist");
    // Tag with ScheduledTaskMarker so the scheduler's serve hook
    // (`take_of::<ScheduledTaskMarker>()`) picks the task up and starts it.
    registry.add_boxed_for::<ScheduledTaskMarker>(vec![counting_task(
        "ticker",
        ScheduleConfig::Interval(Duration::from_millis(50)),
        counter.clone(),
    )]);

    let app = app
        .register_routes(
            r2e_core::http::Router::new().route("/ping", get(|| async { "pong" })),
        )
        .prepare(&addr);

    assert_eq!(app.workers().unwrap(), Some(2));

    let server = tokio::spawn(async move { app.run().await.map_err(|e| e.to_string()) });

    // Wait until the sharded server accepts connections.
    let mut ready = false;
    for _ in 0..100 {
        if request_ping(&addr).await.is_ok() {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ready, "server did not become ready in time");

    // The scheduled task must tick on the control plane while workers serve.
    let mut ticked = false;
    for _ in 0..100 {
        if counter.load(Ordering::SeqCst) >= 2 {
            ticked = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        ticked,
        "scheduled task did not tick while sharded server was running"
    );

    // Trigger graceful shutdown and confirm `run()` returns within 10s.
    tokio::time::sleep(Duration::from_millis(200)).await;
    // SAFETY: raising SIGINT on our own process; tokio has a handler.
    unsafe {
        libc::raise(libc::SIGINT);
    }

    let joined = tokio::time::timeout(Duration::from_secs(10), server).await;
    match joined {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(e))) => panic!("server returned error: {e}"),
        Ok(Err(join_err)) => panic!("server task join error: {join_err}"),
        Err(_) => panic!("server did not shut down within 10s after SIGINT"),
    }
}
