//! `skip_if` predicate on scheduled tasks (Quarkus `skipExecutionIf`).
//!
//! The predicate runs at the start of every tick: `true` suppresses the body,
//! counts in `ScheduledJobInfo::skip_count`, and leaves `run_count`/`last_run`/
//! `last_duration` untouched. The schedule keeps advancing either way.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use r2e_executor::{ExecutorConfig, PoolExecutor};
use r2e_scheduler::{
    start_jobs, ScheduleConfig, ScheduledJobRegistry, ScheduledTask, ScheduledTaskDef,
    SchedulerCommands, SchedulerHandle,
};
use tokio_util::sync::CancellationToken;

type SkipState = (Arc<AtomicUsize>, Arc<AtomicBool>);

/// A 50ms-interval task whose predicate skips while `gate` is `true`.
fn gated_task(runs: Arc<AtomicUsize>, gate: Arc<AtomicBool>) -> ScheduledTaskDef<SkipState> {
    ScheduledTaskDef::new(
        "gated",
        ScheduleConfig::Interval(Duration::from_millis(50)),
        (runs, gate),
        |(runs, _): SkipState| async move {
            runs.fetch_add(1, Ordering::SeqCst);
        },
    )
    .with_skip_if(|(_, gate): SkipState| async move { gate.load(Ordering::SeqCst) })
}

fn boxed_job(task: ScheduledTaskDef<SkipState>) -> r2e_scheduler::ScheduledJob {
    (Box::new(task) as Box<dyn ScheduledTask>).into_job()
}

#[tokio::test]
async fn skip_if_suppresses_ticks_and_counts_skips() {
    let runs = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(AtomicBool::new(true)); // skipping

    let registry = ScheduledJobRegistry::new();
    let cancel = CancellationToken::new();
    let pool = PoolExecutor::new(ExecutorConfig::default());
    start_jobs(
        vec![boxed_job(gated_task(runs.clone(), gate.clone()))],
        cancel.clone(),
        pool,
        registry.clone(),
        SchedulerCommands::disconnected(),
    );

    // Phase 1: predicate true — every tick is skipped, none runs.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(runs.load(Ordering::SeqCst), 0, "all ticks must be skipped");
    let info = registry.job("gated").expect("job registered");
    assert!(
        info.skip_count >= 2,
        "skips recorded, got {}",
        info.skip_count
    );
    assert_eq!(info.run_count, 0, "skipped ticks must not count as runs");
    assert!(
        info.last_run.is_none(),
        "last_run only set when the body runs"
    );
    assert!(
        info.last_duration.is_none(),
        "last_duration only set when the body runs"
    );

    // Phase 2: predicate false — ticks run again and are counted as runs.
    gate.store(false, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(300)).await;
    cancel.cancel();

    assert!(
        runs.load(Ordering::SeqCst) >= 2,
        "ticks must run once the predicate clears"
    );
    let info = registry.job("gated").expect("job registered");
    assert!(info.run_count >= 2, "runs recorded, got {}", info.run_count);
    assert!(info.last_run.is_some());
    assert!(info.last_duration.is_some());
}

#[tokio::test]
async fn skip_if_applies_to_trigger_now() {
    let runs = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(AtomicBool::new(true)); // always skipping

    // Long interval: only the immediate initial tick plus our trigger_now fire.
    let task = ScheduledTaskDef::new(
        "gated_manual",
        ScheduleConfig::Interval(Duration::from_secs(3600)),
        (runs.clone(), gate.clone()),
        |(runs, _): SkipState| async move {
            runs.fetch_add(1, Ordering::SeqCst);
        },
    )
    .with_skip_if(|(_, gate): SkipState| async move { gate.load(Ordering::SeqCst) });

    let registry = ScheduledJobRegistry::new();
    let cancel = CancellationToken::new();
    let (handle, commands) = SchedulerHandle::channel(cancel.clone());
    let pool = PoolExecutor::new(ExecutorConfig::default());
    start_jobs(
        vec![(Box::new(task) as Box<dyn ScheduledTask>).into_job()],
        cancel.clone(),
        pool,
        registry.clone(),
        commands,
    );

    // Let the initial (skipped) tick settle, then fire out of band.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(handle.trigger_now("gated_manual").await);
    tokio::time::sleep(Duration::from_millis(100)).await;
    cancel.cancel();

    assert_eq!(
        runs.load(Ordering::SeqCst),
        0,
        "the predicate also gates trigger_now ticks"
    );
    let info = registry.job("gated_manual").expect("job registered");
    assert!(info.skip_count >= 2, "initial + manual tick both skipped");
    assert_eq!(info.run_count, 0);
}
