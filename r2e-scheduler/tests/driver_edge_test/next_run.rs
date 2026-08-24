use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use r2e_core::rt::CancelToken;
use r2e_scheduler::{ScheduleConfig, ScheduledJobRegistry, ScheduledTask, SchedulerHandle};

use crate::support::{
    await_next_run, await_starts, counting_task, quiet_task, test_pool, tracing_task, TickTrace,
};

#[r2e_core::test(flavor = "multi_thread", worker_threads = 2)]
async fn next_run_is_none_while_a_skip_tick_holds_the_job_off_the_clock() {
    let trace = TickTrace::gated();
    let task = tracing_task(
        "long_tick",
        ScheduleConfig::Interval(r2e_scheduler::PositiveDuration::from_millis(300).unwrap()),
        trace.clone(),
    ); // default OverlapPolicy::Skip

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

    await_starts(&trace, 1, "the first tick must start").await;
    // Observation must be DRIVER-SERIALIZED, not merely post-start: the pool
    // spawns the body from inside `submit_tick`, so "the tick started" alone
    // does not prove the driver reached the bookkeeping. Chosen mechanism: a
    // command roundtrip on an unrelated, never-firing job. The driver has no
    // await inside the due loop and only polls commands between iterations, so
    // this reply proves the pop that spent the deadline has fully executed.
    // (Preferred over poll-until-None, which would poll for the very value the
    // assertion is about.)
    assert!(handle.pause("roundtrip").await, "roundtrip target is known");

    // Mid-tick, gate CLOSED: the deadline that fired is spent, no completion
    // can have replaced it, and `next_run` must say so.
    let mid = registry.job("long_tick").expect("job info");
    assert!(
        mid.next_run.is_none(),
        "a Skip job holds no deadline while its tick runs: `next_run` must not \
         keep advertising the instant that already fired (got {:?}, now {})",
        mid.next_run,
        Utc::now()
    );

    // Completion republishes a fresh, future deadline. `ends` fires inside the
    // pool job, BEFORE the driver's completion arm runs `arm_next`, so no sleep
    // and no command roundtrip can stand in for the publication — wait for it.
    trace.open();
    trace.await_ends(1, "the tick must finish once released").await;
    let next = await_next_run(&registry, "long_tick").await;
    assert!(
        next > Utc::now() - chrono::Duration::milliseconds(50),
        "the republished deadline must be a future fire, not the spent one ({next})"
    );

    cancel.cancel();
    trace.open(); // unblock the drain
    tokio::time::timeout(Duration::from_secs(5), driver)
        .await
        .expect("driver must stop")
        .expect("driver task must not panic");
}

// ── Telemetry outlives no driver ────────────────────────────────────────────
//
// `next_run` is `Some` only while a live driver holds that deadline. The two
// tests below pin both halves: a deadline spent on a submission that fails
// (`PoolClosed` exits the driver) leaves nothing published, and a driver that
// stops for any reason clears what is still armed on its way out.

#[r2e_core::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pool_closed_submission_leaves_no_deadline_published() {
    let counter = Arc::new(AtomicUsize::new(0));
    // Fires immediately: the first pop spends the deadline, and the submission
    // that follows it finds a closed pool.
    let task = counting_task(
        "doomed",
        ScheduleConfig::Interval(r2e_scheduler::PositiveDuration::from_millis(200).unwrap()),
        counter.clone(),
    );

    let registry = ScheduledJobRegistry::new();
    let cancel = CancelToken::new();
    let pool = test_pool();
    pool.shutdown(); // closed before the driver ever pops
    let (_handle, commands) = SchedulerHandle::channel(cancel.clone());
    let boxed: Box<dyn ScheduledTask> = Box::new(task);
    let jobs: Vec<_> = [boxed].into_iter().map(|t| t.into_job()).collect();
    let driver = r2e_core::rt::spawn(r2e_scheduler::jobs_driver(
        jobs,
        cancel.clone(),
        pool,
        registry.clone(),
        commands,
    ));

    // The driver leaves on its own: a closed pool is terminal.
    tokio::time::timeout(Duration::from_secs(5), driver)
        .await
        .expect("a closed pool must stop the driver")
        .expect("driver task must not panic");

    let info = registry.job("doomed").expect("still listed");
    assert!(
        info.next_run.is_none(),
        "the deadline was consumed by a submission that never ran, and the \
         driver is gone: `next_run` must not keep advertising it (got {:?})",
        info.next_run
    );
    assert_eq!(counter.load(Ordering::SeqCst), 0, "nothing ran");
}

#[r2e_core::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stopped_driver_publishes_no_deadline_for_any_job() {
    // A far-future deadline: still armed, on the heap, when cancellation hits.
    let counter = Arc::new(AtomicUsize::new(0));
    let task = counting_task(
        "armed_at_exit",
        ScheduleConfig::IntervalWithDelay {
            interval: r2e_scheduler::PositiveDuration::from_secs(3600).unwrap(),
            initial_delay: Duration::from_secs(3600),
        },
        counter.clone(),
    );

    let registry = ScheduledJobRegistry::new();
    let cancel = CancelToken::new();
    let (_handle, commands) = SchedulerHandle::channel(cancel.clone());
    let boxed: Box<dyn ScheduledTask> = Box::new(task);
    let jobs: Vec<_> = [boxed].into_iter().map(|t| t.into_job()).collect();
    let driver = r2e_core::rt::spawn(r2e_scheduler::jobs_driver(
        jobs,
        cancel.clone(),
        test_pool(),
        registry.clone(),
        commands,
    ));

    await_next_run(&registry, "armed_at_exit").await;

    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), driver)
        .await
        .expect("driver must stop")
        .expect("driver task must not panic");

    let info = registry.job("armed_at_exit").expect("still listed");
    assert!(
        info.next_run.is_none(),
        "the driver that owned this deadline has stopped: nothing will fire it, \
         so telemetry must not advertise it (got {:?})",
        info.next_run
    );
}
