use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use r2e_executor::{ExecutorConfig, JobError, PoolExecutor, RejectedError};
use tokio::sync::Notify;

#[tokio::test]
async fn submit_and_await_returns_result() {
    let exec = PoolExecutor::new(ExecutorConfig::default());
    let handle = exec.submit(async { 7 + 35 });
    let result = handle.await.expect("job should succeed");
    assert_eq!(result, 42);

    let m = exec.metrics();
    assert_eq!(m.completed, 1);
    assert_eq!(m.queued, 0);
    assert_eq!(m.running, 0);
    assert_eq!(m.rejected, 0);
}

#[tokio::test]
async fn concurrent_limit_enforced_by_semaphore() {
    let exec = PoolExecutor::new(ExecutorConfig {
        max_concurrent: 2,
        queue_capacity: 8,
        shutdown_timeout_secs: 5,
    });

    let inflight = Arc::new(AtomicU32::new(0));
    let max_seen = Arc::new(AtomicU32::new(0));
    let release = Arc::new(Notify::new());

    let mut handles = Vec::new();
    for _ in 0..3 {
        let inflight = inflight.clone();
        let max_seen = max_seen.clone();
        let release = release.clone();
        handles.push(exec.submit(async move {
            let cur = inflight.fetch_add(1, Ordering::SeqCst) + 1;
            max_seen.fetch_max(cur, Ordering::SeqCst);
            release.notified().await;
            inflight.fetch_sub(1, Ordering::SeqCst);
        }));
    }

    // Let pending jobs reach steady state.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(exec.metrics().running, 2);
    assert_eq!(exec.metrics().queued, 1);

    // Release all 3 jobs.
    release.notify_waiters();
    // Notify once more after first two finish so third (now running) wakes too.
    tokio::time::sleep(Duration::from_millis(20)).await;
    release.notify_waiters();

    for h in handles {
        h.await.expect("job should succeed");
    }
    assert_eq!(max_seen.load(Ordering::SeqCst), 2, "only 2 jobs may run at once");
}

#[tokio::test]
async fn try_submit_rejects_when_queue_full() {
    let exec = PoolExecutor::new(ExecutorConfig {
        max_concurrent: 1,
        queue_capacity: 1,
        shutdown_timeout_secs: 5,
    });
    let release = Arc::new(Notify::new());

    // Slot 1: running.
    let r1 = release.clone();
    let h1 = exec.try_submit(async move { r1.notified().await }).expect("first slot ok");
    // Slot 2: queued.
    let r2 = release.clone();
    let h2 = exec.try_submit(async move { r2.notified().await }).expect("second slot ok");

    tokio::time::sleep(Duration::from_millis(30)).await;

    // Third must be rejected — cap is max_concurrent + queue_capacity = 2.
    let err = exec.try_submit(async {}).err().expect("third should be rejected");
    assert_eq!(err, RejectedError::QueueFull);
    assert!(exec.metrics().rejected >= 1);

    release.notify_waiters();
    tokio::time::sleep(Duration::from_millis(20)).await;
    release.notify_waiters();
    h1.await.unwrap();
    h2.await.unwrap();
}

#[tokio::test]
async fn graceful_shutdown_drains_running_jobs() {
    let exec = PoolExecutor::new(ExecutorConfig {
        max_concurrent: 4,
        queue_capacity: 8,
        shutdown_timeout_secs: 5,
    });

    let counter = Arc::new(AtomicU32::new(0));
    let mut handles = Vec::new();
    for _ in 0..4 {
        let c = counter.clone();
        handles.push(exec.submit(async move {
            tokio::time::sleep(Duration::from_millis(80)).await;
            c.fetch_add(1, Ordering::SeqCst);
        }));
    }

    // Let all 4 jobs acquire their permits and start running.
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(exec.metrics().running, 4);

    let drained = exec.shutdown_graceful(Duration::from_secs(2)).await;
    assert!(drained, "graceful shutdown should drain within timeout");
    assert!(exec.is_shut_down());

    for h in handles {
        h.await.expect("running jobs finish despite shutdown");
    }
    assert_eq!(counter.load(Ordering::SeqCst), 4);

    // New submissions after shutdown are rejected.
    let post = exec.submit(async { 1 }).await;
    assert!(matches!(post, Err(JobError::Shutdown)));
    let try_post = exec.try_submit(async { 1 });
    assert_eq!(try_post.err(), Some(RejectedError::Shutdown));
}

#[tokio::test]
async fn shutdown_aborts_queued_submissions() {
    let exec = PoolExecutor::new(ExecutorConfig {
        max_concurrent: 1,
        queue_capacity: 8,
        shutdown_timeout_secs: 5,
    });
    let release = Arc::new(Notify::new());

    let r = release.clone();
    let running = exec.submit(async move { r.notified().await; "done" });
    // Queued — never gets a permit because we shut down before releasing.
    let queued = exec.submit(async { "never" });

    tokio::time::sleep(Duration::from_millis(20)).await;
    exec.shutdown();

    // Queued task aborts with Shutdown.
    assert!(matches!(queued.await, Err(JobError::Shutdown)));

    // Allow the running task to finish.
    release.notify_waiters();
    assert_eq!(running.await.unwrap(), "done");
}
