//! Feature C — runtime job control (pause / resume / trigger-now) + stats.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use r2e_executor::{ExecutorConfig, PoolExecutor};
use r2e_scheduler::{
    start_jobs, OverlapPolicy, ScheduleConfig, ScheduledJobRegistry, ScheduledTask,
    ScheduledTaskDef, SchedulerHandle,
};
use tokio_util::sync::CancellationToken;

fn test_pool() -> PoolExecutor {
    PoolExecutor::new(ExecutorConfig::default())
}

/// Wire a single task to the driver with runtime control + a shared registry.
fn spawn(
    task: ScheduledTaskDef<impl Clone + Send + Sync + 'static>,
    registry: ScheduledJobRegistry,
) -> (SchedulerHandle, CancellationToken) {
    let token = CancellationToken::new();
    let (handle, commands) = SchedulerHandle::channel(token.clone());
    let boxed: Box<dyn ScheduledTask> = Box::new(task);
    let jobs: Vec<_> = [boxed].into_iter().map(|t| t.into_job()).collect();
    start_jobs(jobs, token.clone(), test_pool(), registry, commands);
    (handle, token)
}

fn counting_task(
    name: &str,
    schedule: ScheduleConfig,
    counter: Arc<AtomicUsize>,
) -> ScheduledTaskDef<Arc<AtomicUsize>> {
    ScheduledTaskDef::new(name, schedule, counter, |c: Arc<AtomicUsize>| async move {
        c.fetch_add(1, Ordering::SeqCst);
    })
}

#[r2e_core::test]
async fn pause_freezes_execution_and_resume_restarts_it() {
    let counter = Arc::new(AtomicUsize::new(0));
    let task = counting_task(
        "pausable",
        ScheduleConfig::Interval(Duration::from_millis(50)),
        counter.clone(),
    );
    let (handle, token) = spawn(task, ScheduledJobRegistry::new());

    // It ticks for a while.
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(counter.load(Ordering::SeqCst) >= 1, "should have ticked");

    assert!(
        handle.pause("pausable").await,
        "pause of a known job succeeds"
    );
    // Let any in-flight tick settle, then snapshot.
    tokio::time::sleep(Duration::from_millis(80)).await;
    let frozen = counter.load(Ordering::SeqCst);

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        frozen,
        counter.load(Ordering::SeqCst),
        "paused job must not execute"
    );

    assert!(
        handle.resume("pausable").await,
        "resume of a known job succeeds"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        counter.load(Ordering::SeqCst) > frozen,
        "resumed job must run again"
    );

    token.cancel();
}

#[r2e_core::test]
async fn trigger_now_fires_a_job_immediately() {
    let counter = Arc::new(AtomicUsize::new(0));
    // A long delay + long interval means it never fires on its own.
    let task = counting_task(
        "manual",
        ScheduleConfig::IntervalWithDelay {
            interval: Duration::from_secs(3600),
            initial_delay: Duration::from_secs(3600),
        },
        counter.clone(),
    );
    let (handle, token) = spawn(task, ScheduledJobRegistry::new());

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        counter.load(Ordering::SeqCst),
        0,
        "must not fire on its own"
    );

    assert!(
        handle.trigger_now("manual").await,
        "trigger of a known job succeeds"
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "trigger_now must fire the job once"
    );

    // Unknown job → false.
    assert!(!handle.trigger_now("nope").await);

    token.cancel();
}

#[r2e_core::test]
async fn trigger_now_on_in_flight_skip_job_returns_false() {
    let entered = Arc::new(AtomicUsize::new(0));
    let e = entered.clone();
    // Skip policy (default), tick holds for 300ms; never fires on its own.
    let task = ScheduledTaskDef::new(
        "slow",
        ScheduleConfig::IntervalWithDelay {
            interval: Duration::from_secs(3600),
            initial_delay: Duration::from_secs(3600),
        },
        e,
        |e: Arc<AtomicUsize>| async move {
            e.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(300)).await;
        },
    );
    let (handle, token) = spawn(task, ScheduledJobRegistry::new());

    assert!(handle.trigger_now("slow").await, "first trigger submits");
    // Give the tick a moment to enter the pool and mark itself in-flight.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !handle.trigger_now("slow").await,
        "a Skip job already running refuses a second trigger"
    );
    assert_eq!(
        entered.load(Ordering::SeqCst),
        1,
        "only the first trigger produced a running tick"
    );

    token.cancel();
}

#[r2e_core::test]
async fn concurrent_job_always_accepts_trigger_now() {
    let entered = Arc::new(AtomicUsize::new(0));
    let e = entered.clone();
    let task = ScheduledTaskDef::new(
        "conc",
        ScheduleConfig::IntervalWithDelay {
            interval: Duration::from_secs(3600),
            initial_delay: Duration::from_secs(3600),
        },
        e,
        |e: Arc<AtomicUsize>| async move {
            e.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(300)).await;
        },
    )
    .with_overlap(OverlapPolicy::Concurrent);
    let (handle, token) = spawn(task, ScheduledJobRegistry::new());

    assert!(handle.trigger_now("conc").await);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        handle.trigger_now("conc").await,
        "a Concurrent job accepts overlapping triggers"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(entered.load(Ordering::SeqCst), 2, "both triggers ran");

    token.cancel();
}

#[r2e_core::test]
async fn stats_are_populated_and_paused_flag_toggles() {
    let registry = ScheduledJobRegistry::new();
    let counter = Arc::new(AtomicUsize::new(0));
    let task = counting_task(
        "stats",
        ScheduleConfig::Interval(Duration::from_millis(50)),
        counter.clone(),
    );
    let (handle, token) = spawn(task, registry.clone());

    tokio::time::sleep(Duration::from_millis(250)).await;
    let info = registry.job("stats").expect("job registered");
    assert!(
        info.run_count >= 3,
        "run_count should accrue, got {}",
        info.run_count
    );
    assert!(info.last_run.is_some(), "last_run recorded");
    assert!(info.next_run.is_some(), "next_run recorded");
    assert!(info.last_duration.is_some(), "last_duration recorded");
    assert!(!info.paused, "not paused yet");

    assert!(handle.pause("stats").await);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(registry.job("stats").unwrap().paused, "paused flag set");

    assert!(handle.resume("stats").await);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        !registry.job("stats").unwrap().paused,
        "paused flag cleared"
    );

    // Unknown-job control returns false.
    assert!(!handle.pause("ghost").await);
    assert!(!handle.resume("ghost").await);

    token.cancel();
}
