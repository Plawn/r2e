//! Driver edge cases: executor shutdown mid-flight, contained panics, the
//! Skip-overlap-with-an-out-of-band-tick path, and exhausted cron schedules.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, Timelike, Utc};
use r2e_executor::{ExecutorConfig, PoolExecutor};
use r2e_scheduler::{
    start_jobs, OverlapPolicy, ScheduleConfig, ScheduledJobRegistry, ScheduledTask,
    ScheduledTaskDef, SchedulerCommands, SchedulerHandle,
};
use tokio_util::sync::CancellationToken;

fn test_pool() -> PoolExecutor {
    PoolExecutor::new(ExecutorConfig::default())
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

fn start_one(
    task: ScheduledTaskDef<impl Clone + Send + Sync + 'static>,
    cancel: CancellationToken,
    pool: PoolExecutor,
    registry: ScheduledJobRegistry,
) {
    let boxed: Box<dyn ScheduledTask> = Box::new(task);
    let jobs: Vec<_> = [boxed].into_iter().map(|t| t.into_job()).collect();
    start_jobs(
        jobs,
        cancel,
        pool,
        registry,
        SchedulerCommands::disconnected(),
    );
}

// ── Executor shut down before the driver submits ────────────────────────────
//
// The pool is shut down first, so the very first (immediate) interval fire
// fails to submit: `submit_tick` returns false and the driver stops. The body
// never runs.

#[r2e_core::test]
async fn skip_job_stops_driver_when_executor_is_shut_down() {
    let counter = Arc::new(AtomicUsize::new(0));
    let pool = test_pool();
    pool.shutdown(); // submissions now rejected

    let cancel = CancellationToken::new();
    let task = counting_task(
        "skip_dead_pool",
        ScheduleConfig::Interval(r2e_scheduler::PositiveDuration::from_millis(20).unwrap()),
        counter.clone(),
    ); // default OverlapPolicy::Skip
    start_one(task, cancel.clone(), pool, ScheduledJobRegistry::new());

    tokio::time::sleep(Duration::from_millis(150)).await;
    cancel.cancel();

    assert_eq!(
        counter.load(Ordering::SeqCst),
        0,
        "body must never run when the pool rejects submissions"
    );
}

#[r2e_core::test]
async fn concurrent_job_stops_driver_when_executor_is_shut_down() {
    let counter = Arc::new(AtomicUsize::new(0));
    let pool = test_pool();
    pool.shutdown();

    let cancel = CancellationToken::new();
    let task = counting_task(
        "concurrent_dead_pool",
        ScheduleConfig::Interval(r2e_scheduler::PositiveDuration::from_millis(20).unwrap()),
        counter.clone(),
    )
    .with_overlap(OverlapPolicy::Concurrent);
    start_one(task, cancel.clone(), pool, ScheduledJobRegistry::new());

    tokio::time::sleep(Duration::from_millis(150)).await;
    cancel.cancel();

    assert_eq!(
        counter.load(Ordering::SeqCst),
        0,
        "body must never run when the pool rejects submissions"
    );
}

// ── A panicking tick is contained and counted ───────────────────────────────

#[r2e_core::test]
async fn panicking_tick_increments_panic_count() {
    let registry = ScheduledJobRegistry::new();
    let cancel = CancellationToken::new();

    // Struct-literal form with the future cast to `Output = ()` so the
    // diverging (`panic!`) body doesn't trip never-type inference.
    let task = ScheduledTaskDef {
        overlap: OverlapPolicy::Skip,
        skip: None,
        name: "panicker".to_string(),
        schedule: ScheduleConfig::Interval(r2e_scheduler::PositiveDuration::from_millis(50).unwrap()),
        state: (),
        task: Box::new(|()| {
            Box::pin(async move {
                panic!("intentional panic in tick");
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        }),
    };
    start_one(task, cancel.clone(), test_pool(), registry.clone());

    tokio::time::sleep(Duration::from_millis(300)).await;
    cancel.cancel();

    let info = registry.job("panicker").expect("job registered");
    assert!(
        info.panic_count >= 1,
        "at least one contained panic should be recorded, got {}",
        info.panic_count
    );
}

// ── Skip overlap with an out-of-band (trigger_now) tick still running ────────
//
// An initial delay keeps the scheduled fire pending in the future. A
// `trigger_now` submits a long-running out-of-band tick; when the scheduled
// deadlines then arrive they see `in_flight > 0` and are skipped (cadence still
// advances) rather than piling up.

#[r2e_core::test]
async fn skip_scheduled_ticks_yield_to_an_in_flight_oob_tick() {
    let runs = Arc::new(AtomicUsize::new(0));
    let registry = ScheduledJobRegistry::new();
    let cancel = CancellationToken::new();
    let (handle, commands) = SchedulerHandle::channel(cancel.clone());

    let r = runs.clone();
    let task = ScheduledTaskDef::new(
        "overlap_skip",
        // First scheduled fire is 150ms out; then every 100ms.
        ScheduleConfig::IntervalWithDelay {
            interval: r2e_scheduler::PositiveDuration::from_millis(100).unwrap(),
            initial_delay: Duration::from_millis(150),
        },
        r,
        |r: Arc<AtomicUsize>| async move {
            r.fetch_add(1, Ordering::SeqCst);
            // Outlasts several scheduled deadlines.
            tokio::time::sleep(Duration::from_millis(400)).await;
        },
    ); // default Skip
    let boxed: Box<dyn ScheduledTask> = Box::new(task);
    let jobs: Vec<_> = [boxed].into_iter().map(|t| t.into_job()).collect();
    start_jobs(jobs, cancel.clone(), test_pool(), registry.clone(), commands);

    // Fire out of band while the scheduled entry is still pending in the future.
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(handle.trigger_now("overlap_skip").await, "OOB tick submits");

    // The scheduled deadlines at ~150ms and ~250ms fall while the OOB tick
    // (running until ~420ms) is in flight, so they are skipped. Stop before the
    // OOB tick finishes so no further scheduled tick can run.
    tokio::time::sleep(Duration::from_millis(280)).await;
    cancel.cancel();

    assert_eq!(
        runs.load(Ordering::SeqCst),
        1,
        "only the out-of-band tick ran; overlapping scheduled ticks were skipped"
    );
    // Cadence kept advancing while ticks were skipped.
    let info = registry.job("overlap_skip").expect("registered");
    assert!(info.next_run.is_some(), "schedule kept advancing");
}

// ── Cron with no upcoming occurrences ────────────────────────────────────────

#[r2e_core::test]
async fn cron_pinned_to_the_past_never_arms() {
    // A fully-pinned cron in the past (year 2000) yields no upcoming fire, so
    // `cron_next_instant` returns None at initial arming and the job is dormant.
    let counter = Arc::new(AtomicUsize::new(0));
    let registry = ScheduledJobRegistry::new();
    let cancel = CancellationToken::new();
    let task = counting_task(
        "past_cron",
        ScheduleConfig::Cron("0 0 0 1 1 * 2000".to_string()),
        counter.clone(),
    );
    start_one(task, cancel.clone(), test_pool(), registry.clone());

    tokio::time::sleep(Duration::from_millis(200)).await;
    cancel.cancel();

    assert_eq!(counter.load(Ordering::SeqCst), 0, "past cron never fires");
    let info = registry.job("past_cron").expect("registered");
    assert!(info.next_run.is_none(), "no upcoming fire");
}

#[r2e_core::test(flavor = "multi_thread", worker_threads = 2)]
async fn cron_exhausts_after_its_single_occurrence() {
    // Build a cron pinned to a single wall-clock second ~2s in the future.
    // It fires exactly once; on re-arm the schedule is exhausted, exercising
    // the "no more upcoming cron executions" branch.
    let fire = Utc::now() + chrono::Duration::seconds(2);
    let expr = format!(
        "{} {} {} {} {} * {}",
        fire.second(),
        fire.minute(),
        fire.hour(),
        fire.day(),
        fire.month(),
        fire.year(),
    );

    let counter = Arc::new(AtomicUsize::new(0));
    let registry = ScheduledJobRegistry::new();
    let cancel = CancellationToken::new();
    let task = counting_task(
        "one_shot_cron",
        ScheduleConfig::Cron(expr),
        counter.clone(),
    );
    start_one(task, cancel.clone(), test_pool(), registry.clone());

    // Wait past the single occurrence + re-arm.
    tokio::time::sleep(Duration::from_millis(3800)).await;
    cancel.cancel();

    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "the pinned cron fires exactly once"
    );
    let info = registry.job("one_shot_cron").expect("registered");
    assert!(
        info.next_run.is_none(),
        "schedule is exhausted after its only occurrence"
    );
}
