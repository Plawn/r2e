//! Full serve lifecycle: the scheduler's `on_serve` hook starts tasks and, in
//! dedicated-pool mode, its `on_shutdown_async` hook drains the private pool on
//! graceful stop. Driven with `StopHandle` (no OS signal).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use r2e_core::builder::{ScheduledTaskMarker, TaskRegistryHandle};
use r2e_core::config::R2eConfig;
use r2e_core::http::routing::get;
use r2e_core::http::Router;
use r2e_core::AppBuilder;
use r2e_executor::Executor;
use r2e_scheduler::{ScheduleConfig, ScheduledTask, ScheduledTaskDef, Scheduler};

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    port
}

fn counting_task(
    name: &str,
    schedule: ScheduleConfig,
    counter: Arc<AtomicUsize>,
) -> Box<dyn std::any::Any + Send> {
    let def = ScheduledTaskDef::new(name, schedule, counter, |c: Arc<AtomicUsize>| async move {
        c.fetch_add(1, Ordering::SeqCst);
    });
    let trait_obj: Box<dyn ScheduledTask> = Box::new(def);
    Box::new(trait_obj)
}

// ── Dedicated pool: serve starts tasks, stop drains the private pool ─────────

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn dedicated_pool_serve_starts_tasks_and_drains_on_stop() {
    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    // Dedicated pool with a non-zero graceful timeout (exercises the async
    // shutdown drain's `shutdown_graceful` arm).
    let yaml = format!(
        "server:\n  port: {port}\nscheduler:\n  executor: dedicated\n  shutdown-timeout: 2s\n"
    );
    let config = R2eConfig::from_yaml_str(&yaml).unwrap();

    let counter = Arc::new(AtomicUsize::new(0));

    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(Scheduler)
        .plugin(Executor)
        .build_state()
        .await;

    let registry = app
        .get_plugin_data::<TaskRegistryHandle>()
        .expect("scheduler registry should exist");
    registry.add_boxed_for::<ScheduledTaskMarker>(vec![counting_task(
        "served",
        ScheduleConfig::Interval(r2e_scheduler::PositiveDuration::from_millis(50).unwrap()),
        counter.clone(),
    )]);

    let app = app
        .register_routes(Router::new().route("/ping", get(|| async { "pong" })))
        .prepare(&addr);
    let stop = app.stop_handle();
    let server = tokio::spawn(async move { app.run().await.map_err(|e| e.to_string()) });

    // The serve hook must have started the task on the dedicated pool.
    let mut ticked = false;
    for _ in 0..100 {
        if counter.load(Ordering::SeqCst) >= 1 {
            ticked = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ticked, "scheduled task did not tick under serve");

    // Graceful stop: drives the dedicated pool's on_shutdown_async drain.
    stop.stop();
    match tokio::time::timeout(Duration::from_secs(10), server).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(e))) => panic!("server returned error: {e}"),
        Ok(Err(join_err)) => panic!("server task join error: {join_err}"),
        Err(_) => panic!("server did not shut down within 10s of stop()"),
    }
}

// ── Dedicated pool with a zero graceful timeout drains immediately ───────────

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn dedicated_pool_zero_timeout_drains_immediately_on_stop() {
    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    // shutdown-timeout: 0s exercises the immediate `shutdown()` drain arm.
    let yaml = format!(
        "server:\n  port: {port}\nscheduler:\n  executor: dedicated\n  shutdown-timeout: 0s\n"
    );
    let config = R2eConfig::from_yaml_str(&yaml).unwrap();

    let counter = Arc::new(AtomicUsize::new(0));

    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(Scheduler)
        .plugin(Executor)
        .build_state()
        .await;

    let registry = app
        .get_plugin_data::<TaskRegistryHandle>()
        .expect("scheduler registry should exist");
    registry.add_boxed_for::<ScheduledTaskMarker>(vec![counting_task(
        "served_zero",
        ScheduleConfig::Interval(r2e_scheduler::PositiveDuration::from_millis(50).unwrap()),
        counter.clone(),
    )]);

    let app = app
        .register_routes(Router::new().route("/ping", get(|| async { "pong" })))
        .prepare(&addr);
    let stop = app.stop_handle();
    let server = tokio::spawn(async move { app.run().await.map_err(|e| e.to_string()) });

    let mut ticked = false;
    for _ in 0..100 {
        if counter.load(Ordering::SeqCst) >= 1 {
            ticked = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ticked, "scheduled task did not tick under serve");

    stop.stop();
    match tokio::time::timeout(Duration::from_secs(10), server).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(e))) => panic!("server returned error: {e}"),
        Ok(Err(join_err)) => panic!("server task join error: {join_err}"),
        Err(_) => panic!("server did not shut down within 10s of stop()"),
    }
}

// ── No scheduled tasks: the serve hook returns early ─────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn serve_with_no_scheduled_tasks_boots_and_stops_cleanly() {
    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    let config = R2eConfig::from_yaml_str(&format!("server:\n  port: {port}\n")).unwrap();

    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(Scheduler)
        .plugin(Executor)
        .build_state()
        .await;

    // No tasks registered: the serve hook calls into task startup and returns
    // early because the extracted task list is empty.
    let app = app
        .register_routes(Router::new().route("/ping", get(|| async { "pong" })))
        .prepare(&addr);
    let stop = app.stop_handle();
    let server = tokio::spawn(async move { app.run().await.map_err(|e| e.to_string()) });

    // Give the serve hook a chance to run, then stop.
    tokio::time::sleep(Duration::from_millis(200)).await;
    stop.stop();
    match tokio::time::timeout(Duration::from_secs(10), server).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(e))) => panic!("server returned error: {e}"),
        Ok(Err(join_err)) => panic!("server task join error: {join_err}"),
        Err(_) => panic!("server did not shut down within 10s of stop()"),
    }
}
