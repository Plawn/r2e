//! Feature A — self-overlap policy (`OverlapPolicy`).
//!
//! A `Concurrent` job whose tick outlasts its cadence accumulates overlapping
//! executions; a `Skip` job under the same load never does (the latter is also
//! covered by `scheduler_test::per_job_ticks_never_overlap`).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use r2e_executor::{ExecutorConfig, PoolExecutor};
use r2e_scheduler::{
    start_jobs, OverlapPolicy, ScheduleConfig, ScheduledJobRegistry, ScheduledTask,
    ScheduledTaskDef, SchedulerCommands,
};
use tokio_util::sync::CancellationToken;

fn test_pool() -> PoolExecutor {
    PoolExecutor::new(ExecutorConfig::default())
}

fn start(task: ScheduledTaskDef<impl Clone + Send + Sync + 'static>) -> CancellationToken {
    let token = CancellationToken::new();
    let boxed: Box<dyn ScheduledTask> = Box::new(task);
    let jobs: Vec<_> = [boxed].into_iter().map(|t| t.into_job()).collect();
    start_jobs(
        jobs,
        token.clone(),
        test_pool(),
        ScheduledJobRegistry::new(),
        SchedulerCommands::disconnected(),
    );
    token
}

/// A 50ms-cadence job whose tick sleeps 200ms must overlap with itself when the
/// policy is `Concurrent` — the observed concurrent-execution gauge reaches >= 2.
#[r2e_core::test]
async fn concurrent_job_overlaps_with_itself() {
    let live = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));

    let l = live.clone();
    let m = max_seen.clone();
    let task = ScheduledTaskDef::new(
        "concurrent",
        ScheduleConfig::Interval(Duration::from_millis(50)),
        (l, m),
        |(l, m): (Arc<AtomicUsize>, Arc<AtomicUsize>)| async move {
            let now = l.fetch_add(1, Ordering::SeqCst) + 1;
            m.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(200)).await;
            l.fetch_sub(1, Ordering::SeqCst);
        },
    )
    .with_overlap(OverlapPolicy::Concurrent);

    let token = start(task);
    tokio::time::sleep(Duration::from_millis(500)).await;
    token.cancel();

    let peak = max_seen.load(Ordering::SeqCst);
    assert!(
        peak >= 2,
        "concurrent job should accumulate overlapping executions, peak was {peak}"
    );
}

/// The same load under the default `Skip` policy must never overlap.
#[r2e_core::test]
async fn skip_job_never_overlaps_under_load() {
    let live = Arc::new(AtomicUsize::new(0));
    let saw_overlap = Arc::new(AtomicBool::new(false));

    let l = live.clone();
    let o = saw_overlap.clone();
    let task = ScheduledTaskDef::new(
        "skip",
        ScheduleConfig::Interval(Duration::from_millis(50)),
        (l, o),
        |(l, o): (Arc<AtomicUsize>, Arc<AtomicBool>)| async move {
            if l.fetch_add(1, Ordering::SeqCst) + 1 > 1 {
                o.store(true, Ordering::SeqCst);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
            l.fetch_sub(1, Ordering::SeqCst);
        },
    ); // default OverlapPolicy::Skip

    let token = start(task);
    tokio::time::sleep(Duration::from_millis(500)).await;
    token.cancel();

    assert!(
        !saw_overlap.load(Ordering::SeqCst),
        "a Skip job's ticks must never overlap with themselves"
    );
}
