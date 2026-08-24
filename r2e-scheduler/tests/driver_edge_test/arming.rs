use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, Timelike, Utc};
use r2e_core::rt::CancelToken;
use r2e_scheduler::{ScheduleConfig, ScheduledJobRegistry, ScheduledTask, SchedulerHandle};

use crate::support::{await_completion_processed, counting_task, quiet_task, test_pool};

// ── Cron with no upcoming occurrences ────────────────────────────────────────

#[r2e_core::test]
async fn cron_pinned_to_the_past_never_arms() {
    // A fully-pinned cron in the past (year 2000) yields no upcoming fire, so
    // `cron_next_instant` returns None at initial arming and the job is dormant.
    let counter = Arc::new(AtomicUsize::new(0));
    let registry = ScheduledJobRegistry::new();
    let cancel = CancelToken::new();
    let (handle, commands) = SchedulerHandle::channel(cancel.clone());
    let task = counting_task(
        "past_cron",
        ScheduleConfig::Cron("0 0 0 1 1 * 2000".to_string()),
        counter.clone(),
    );
    let jobs: Vec<_> = [
        Box::new(task) as Box<dyn ScheduledTask>,
        Box::new(quiet_task("roundtrip")),
    ]
    .into_iter()
    .map(|t| t.into_job())
    .collect();
    let driver = r2e_core::rt::spawn(r2e_scheduler::jobs_driver(
        jobs,
        cancel.clone(),
        test_pool(),
        registry.clone(),
        commands,
    ));

    // Initial arming runs BEFORE the select loop, so a command reply proves the
    // driver made — and published — the arming decision for every job. Observe
    // here, with the driver alive: a read taken after `cancel()` would be worth
    // nothing, because the exit path clears every deadline and would erase an
    // erroneously armed `Some` before the assert could see it.
    assert!(handle.pause("roundtrip").await, "roundtrip target is known");

    let info = registry.job("past_cron").expect("registered");
    assert!(
        info.next_run.is_none(),
        "no upcoming fire (got {:?})",
        info.next_run
    );
    assert_eq!(counter.load(Ordering::SeqCst), 0, "past cron never fires");

    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), driver)
        .await
        .expect("driver must stop")
        .expect("driver task must not panic");
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
    let cancel = CancelToken::new();
    let (handle, commands) = SchedulerHandle::channel(cancel.clone());
    let task = counting_task("one_shot_cron", ScheduleConfig::Cron(expr), counter.clone());
    let jobs: Vec<_> = [
        Box::new(task) as Box<dyn ScheduledTask>,
        Box::new(quiet_task("roundtrip")),
    ]
    .into_iter()
    .map(|t| t.into_job())
    .collect();
    let driver = r2e_core::rt::spawn(r2e_scheduler::jobs_driver(
        jobs,
        cancel.clone(),
        test_pool(),
        registry.clone(),
        commands,
    ));

    // The body ran…
    tokio::time::timeout(Duration::from_secs(10), async {
        while counter.load(Ordering::SeqCst) < 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the pinned cron must fire once");

    // …and the driver PROCESSED that completion, i.e. ran the re-arm whose only
    // correct outcome is "nothing left to arm". Without this barrier the old
    // sleep-then-cancel shape could pass for the wrong reason twice over: the
    // completion might not have been processed at all, and a deadline wrongly
    // published by it would have been erased by the driver's exit clear-all
    // before the assert looked.
    await_completion_processed(&registry, &handle, "one_shot_cron", "roundtrip").await;

    let info = registry.job("one_shot_cron").expect("registered");
    assert!(
        info.next_run.is_none(),
        "schedule is exhausted after its only occurrence (got {:?})",
        info.next_run
    );
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "the pinned cron fires exactly once"
    );

    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), driver)
        .await
        .expect("driver must stop")
        .expect("driver task must not panic");
}

// ── An unrepresentable initial delay must not panic the driver ─────────────
//
// `initial_delay` is a plain `Duration` (no `PositiveDuration` upper bound), and
// initial arming happens before any tick exists — outside the tick factory's
// catch_unwind and before the drain.

#[r2e_core::test]
async fn an_unrepresentable_initial_delay_leaves_the_job_unarmed() {
    let runs = Arc::new(AtomicUsize::new(0));
    let task = counting_task(
        "never_armed",
        ScheduleConfig::IntervalWithDelay {
            interval: r2e_scheduler::PositiveDuration::from_millis(50).unwrap(),
            initial_delay: Duration::MAX,
        },
        runs.clone(),
    );

    let registry = ScheduledJobRegistry::new();
    let cancel = CancelToken::new();
    let (handle, commands) = SchedulerHandle::channel(cancel.clone());
    let jobs: Vec<_> = [
        Box::new(task) as Box<dyn ScheduledTask>,
        Box::new(quiet_task("roundtrip")),
    ]
    .into_iter()
    .map(|t| t.into_job())
    .collect();
    let driver = r2e_core::rt::spawn(r2e_scheduler::jobs_driver(
        jobs,
        cancel.clone(),
        test_pool(),
        registry.clone(),
        commands,
    ));

    // Driver-serialized instead of "sleep and hope": the reply proves the
    // pre-loop arming pass finished, so a deadline this job should never hold
    // would already be published — and the read happens while the driver is
    // alive, before its exit clear-all could hide one.
    assert!(handle.pause("roundtrip").await, "roundtrip target is known");
    let info = registry
        .job("never_armed")
        .expect("the driver must survive arming a job with an unrepresentable initial delay");
    assert!(
        info.next_run.is_none(),
        "an unrepresentable first fire must leave the job exhausted, not armed: {info:?}"
    );
    assert_eq!(
        runs.load(Ordering::SeqCst),
        0,
        "an unarmed job must never fire"
    );

    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), driver)
        .await
        .expect("driver must stop")
        .expect("the driver task must not panic while arming an absurd initial delay");
}
