use std::sync::atomic::Ordering;
use std::time::Duration;

use r2e_scheduler::{start_jobs, ScheduleConfig, ScheduledJobRegistry, ScheduledTask, SchedulerHandle};
use tokio_util::sync::CancellationToken;

use crate::support::{await_next_run, await_starts, test_pool, tracing_task, TickTrace};

// ── Skip overlap with an out-of-band (trigger_now) tick still running ────────
//
// An initial delay keeps the scheduled fire pending in the future. A
// `trigger_now` submits a long-running out-of-band tick; when the scheduled
// deadlines then arrive they see `in_flight > 0` and are skipped (cadence still
// advances) rather than piling up.

#[r2e_core::test]
async fn skip_scheduled_ticks_yield_to_an_in_flight_oob_tick() {
    // Gated instead of "a body sleep long enough": the out-of-band tick stays
    // in flight until this test releases it, so the scheduled deadlines
    // provably fall while it runs however slow the machine is.
    let trace = TickTrace::gated();
    let registry = ScheduledJobRegistry::new();
    let cancel = CancellationToken::new();
    let (handle, commands) = SchedulerHandle::channel(cancel.clone());

    let task = tracing_task(
        "overlap_skip",
        // First scheduled fire is 150ms out; then every 100ms.
        ScheduleConfig::IntervalWithDelay {
            interval: r2e_scheduler::PositiveDuration::from_millis(100).unwrap(),
            initial_delay: Duration::from_millis(150),
        },
        trace.clone(),
    ); // default Skip
    let boxed: Box<dyn ScheduledTask> = Box::new(task);
    let jobs: Vec<_> = [boxed].into_iter().map(|t| t.into_job()).collect();
    start_jobs(
        jobs,
        cancel.clone(),
        test_pool(),
        registry.clone(),
        commands,
    );

    // Fire out of band while the scheduled entry is still pending in the future.
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(handle.trigger_now("overlap_skip").await, "OOB tick submits");
    await_starts(&trace, 1, "the out-of-band tick must start").await;

    // The scheduled deadlines at ~150ms and ~250ms fall while the gated OOB
    // tick is in flight, so they are skipped.
    tokio::time::sleep(Duration::from_millis(280)).await;

    // Cadence kept advancing while ticks were skipped. A skipped pop clears and
    // re-arms in consecutive statements, so `None` is a truthful microsecond
    // transient there — poll rather than sample. Read BEFORE cancelling: a
    // stopped driver clears every published deadline.
    await_next_run(&registry, "overlap_skip").await;

    assert_eq!(
        trace.starts.load(Ordering::SeqCst),
        1,
        "only the out-of-band tick ran; overlapping scheduled ticks were skipped"
    );

    cancel.cancel();
    trace.open(); // unblock the drain
}
