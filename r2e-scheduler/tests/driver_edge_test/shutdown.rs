use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use r2e_core::rt::CancelToken;
use r2e_scheduler::{
    OverlapPolicy, ScheduleConfig, ScheduledJobRegistry, ScheduledTask, ScheduledTaskDef,
    SchedulerCommands, SchedulerHandle,
};

use crate::support::{counting_task, start_one, test_pool};

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

    let cancel = CancelToken::new();
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

    let cancel = CancelToken::new();
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

// ── The driver leaves through the drain, including on executor rejection ────
//
// A tick already in flight owns the serve-scope graph handle. If the driver
// returns while that tick is still running, the tracked driver future ends,
// the graph can be dropped, and the tick keeps touching beans it no longer
// keeps alive. Every exit of the driver loop — cancellation AND "the pool
// refused my submission" — must fall through to the common drain.

#[r2e_core::test]
async fn executor_rejection_still_awaits_the_tick_already_in_flight() {
    let started = Arc::new(tokio::sync::Notify::new());
    let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let payload = (started.clone(), finished.clone());
    let task = ScheduledTaskDef::new(
        "long_tick_vs_dead_pool",
        ScheduleConfig::Interval(r2e_scheduler::PositiveDuration::from_millis(50).unwrap()),
        payload,
        |(started, finished): (Arc<tokio::sync::Notify>, Arc<std::sync::atomic::AtomicBool>)| async move {
            started.notify_one();
            tokio::time::sleep(Duration::from_millis(400)).await;
            finished.store(true, Ordering::SeqCst);
        },
    )
    .with_overlap(OverlapPolicy::Concurrent);

    let pool = test_pool();
    let boxed: Box<dyn ScheduledTask> = Box::new(task);
    let jobs: Vec<_> = [boxed].into_iter().map(|t| t.into_job()).collect();
    let driver = r2e_core::rt::spawn(r2e_scheduler::jobs_driver(
        jobs,
        CancelToken::new(),
        pool.clone(),
        ScheduledJobRegistry::new(),
        SchedulerCommands::disconnected(),
    ));

    // The immediate first fire is in flight; close the pool under the driver's
    // feet so the NEXT submission (50 ms later) is rejected.
    started.notified().await;
    pool.shutdown();

    tokio::time::timeout(Duration::from_secs(5), driver)
        .await
        .expect("the driver must finish once the pool is closed")
        .expect("driver task must not panic");

    assert!(
        finished.load(Ordering::SeqCst),
        "the driver returned while a tick was still running: the executor-rejection \
         path must break into the common drain, not return early"
    );
}

// ── A runtime command issued from inside a tick during shutdown ─────────────
//
// The driver stops polling `command_rx` once it starts draining. If it kept
// the receiver alive, a tick body calling `pause`/`trigger_now` while being
// drained would await a oneshot reply from a driver that is awaiting that very
// tick — a deadlock bounded only by `shutdown_grace_period`. The receiver is
// dropped BEFORE the drain, so commands resolve to `false` immediately.

#[r2e_core::test]
async fn a_command_issued_from_a_tick_during_shutdown_does_not_deadlock() {
    let started = Arc::new(tokio::sync::Notify::new());
    let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let paused_ok = Arc::new(std::sync::atomic::AtomicBool::new(true));

    let cancel = CancelToken::new();
    let (handle, commands) = SchedulerHandle::channel(cancel.clone());

    let payload = (
        handle.clone(),
        started.clone(),
        finished.clone(),
        paused_ok.clone(),
    );
    type Payload = (
        SchedulerHandle,
        Arc<tokio::sync::Notify>,
        Arc<std::sync::atomic::AtomicBool>,
        Arc<std::sync::atomic::AtomicBool>,
    );
    let task = ScheduledTaskDef::new(
        "command_from_tick",
        ScheduleConfig::Interval(r2e_scheduler::PositiveDuration::from_millis(50).unwrap()),
        payload,
        |(handle, started, finished, paused_ok): Payload| async move {
            started.notify_one();
            // Give the test time to start the shutdown while we are in flight.
            tokio::time::sleep(Duration::from_millis(100)).await;
            let ok = handle.pause("command_from_tick").await;
            paused_ok.store(ok, Ordering::SeqCst);
            finished.store(true, Ordering::SeqCst);
        },
    )
    .with_overlap(OverlapPolicy::Concurrent);

    let boxed: Box<dyn ScheduledTask> = Box::new(task);
    let jobs: Vec<_> = [boxed].into_iter().map(|t| t.into_job()).collect();
    let driver = r2e_core::rt::spawn(r2e_scheduler::jobs_driver(
        jobs,
        cancel.clone(),
        test_pool(),
        ScheduledJobRegistry::new(),
        commands,
    ));

    started.notified().await;
    cancel.cancel();

    tokio::time::timeout(Duration::from_secs(5), driver)
        .await
        .expect(
            "the driver deadlocked: a tick awaiting a command reply while the driver \
             awaits that tick",
        )
        .expect("driver task must not panic");

    assert!(
        finished.load(Ordering::SeqCst),
        "the tick must run to completion during the drain"
    );
    assert!(
        !paused_ok.load(Ordering::SeqCst),
        "a runtime command issued once shutdown started must report false, not hang"
    );
}

// ── No new work may start once cancellation is observable ───────────────────
//
// Commands are queued before the driver's first poll and the token is cancelled
// before it starts, so its very first `select!` sees a ready command arm AND a
// cancelled token. Cancellation has priority (`biased`) and the command arm
// refuses anything it dequeues after cancellation, so `trigger_now` cannot
// submit a tick into a shutdown that already began.

#[r2e_core::test]
async fn a_command_queued_before_cancellation_cannot_start_a_tick() {
    // Repeated: with an unbiased select the two ready arms are picked at
    // random, so a single round would let the bug hide half the time.
    for round in 0..12 {
        let runs = Arc::new(AtomicUsize::new(0));
        let cancel = CancelToken::new();
        let (handle, commands) = SchedulerHandle::channel(cancel.clone());

        let task = counting_task(
            "idle_job",
            // Far enough away that no cadence tick can fire during the test:
            // the only way `runs` moves is an accepted TriggerNow.
            ScheduleConfig::Interval(r2e_scheduler::PositiveDuration::from_secs(3600).unwrap()),
            runs.clone(),
        )
        .with_overlap(OverlapPolicy::Concurrent);

        let entered = Arc::new(AtomicUsize::new(0));
        let mut triggers = Vec::new();
        for _ in 0..4 {
            let h = handle.clone();
            let entered = entered.clone();
            triggers.push(r2e_core::rt::spawn(async move {
                entered.fetch_add(1, Ordering::SeqCst);
                h.trigger_now("idle_job").await
            }));
        }
        while entered.load(Ordering::SeqCst) < 4 {
            tokio::task::yield_now().await;
        }
        // The driver does not exist yet, so nothing drains the channel: the
        // commands are sitting in it when the token is cancelled.
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel.cancel();

        let boxed: Box<dyn ScheduledTask> = Box::new(task);
        let jobs: Vec<_> = [boxed].into_iter().map(|t| t.into_job()).collect();
        let driver = r2e_core::rt::spawn(r2e_scheduler::jobs_driver(
            jobs,
            cancel.clone(),
            test_pool(),
            ScheduledJobRegistry::new(),
            commands,
        ));

        tokio::time::timeout(Duration::from_secs(5), driver)
            .await
            .expect("the driver must stop on an already-cancelled token")
            .expect("driver task must not panic");

        for t in triggers {
            let ok = t.await.expect("trigger task must not panic");
            assert!(
                !ok,
                "round {round}: trigger_now must report false once shutdown started"
            );
        }
        assert_eq!(
            runs.load(Ordering::SeqCst),
            0,
            "round {round}: a command queued before cancellation must not submit a new tick"
        );
    }
}
