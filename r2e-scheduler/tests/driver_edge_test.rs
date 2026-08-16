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
use tokio::sync::Semaphore;
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

/// Wait until the job publishes a deadline, and return it.
///
/// The ONLY honest way to observe `next_run` on the far side of a tick
/// completion. Three facts make a bare read (even after a sleep, even after a
/// command roundtrip) a flake:
///
/// 1. A test observes a tick's end through a counter incremented INSIDE the
///    pool job — before the future returns, before the `JoinHandle` resolves,
///    and before the driver's completion arm runs `arm_next`. "The body
///    finished" therefore says nothing about the driver's bookkeeping.
/// 2. A command roundtrip does not serialize past that completion either. The
///    `biased` select does poll the in-flight arm (2) before the command arm
///    (3), but priority only decides between arms that are BOTH ready — and at
///    that instant the completion may simply not be ready yet, so the driver
///    can legitimately answer the command first. (The roundtrip is only valid
///    the other way round, against work the driver has already *done*: see the
///    mid-tick observation in `next_run_is_none_while_a_skip_tick_holds_the_job_off_the_clock`.)
/// 3. Since the deadline is cleared at the pop that spends it, `None` is a
///    truthful transient at every fire — a point sample can land in it.
///
/// Polling for `Some` is not circular the way polling for `None` would be: only
/// the driver ever publishes a deadline, it does so exactly once per re-arm,
/// and no other actor can produce the value the assertion is waiting for.
async fn await_next_run(registry: &ScheduledJobRegistry, name: &str) -> chrono::DateTime<Utc> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            // A job the driver has not registered yet counts as "no deadline
            // published yet", not as a failure: registration happens inside the
            // driver task, so a test that polls immediately after spawning it
            // can legitimately arrive first.
            if let Some(at) = registry.job(name).and_then(|i| i.next_run) {
                return at;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{name}: the driver published no deadline"))
}

/// Wait until the driver has PROCESSED a tick completion for `name`, then
/// serialize behind one further driver iteration.
///
/// The barrier a `None` assert needs on the far side of a tick. An `is_none()`
/// read is NOT the conservative direction it looks like: since the driver
/// clears every published deadline on its way out, a *defective* `Some` gets
/// wiped by cancellation, and an assert taken after a sleep + `cancel()` then
/// passes exactly when the bug is present. Every such assert must therefore be
/// sequenced after a positive, driver-produced event proving the driver already
/// reached the point where the erroneous `Some` would have appeared — and taken
/// while the driver is still alive.
///
/// The event used here is `last_duration`: the driver writes it itself, in the
/// completion arm of the select loop, unlike the tick counters, which the body
/// increments from inside the pool job. Seeing it proves the driver entered the
/// arm — but not that it got past the `arm_next` that follows, since the two
/// writes take the registry lock separately. One command roundtrip closes that
/// gap: the command arm can only run in a LATER loop iteration than the
/// completion arm just witnessed, so its reply proves `arm_next` has had its
/// chance to publish. (The roundtrip is used in the only direction it is valid:
/// against work the driver has already begun — see `await_next_run` for why it
/// cannot be used to await a completion in the first place.)
///
/// `roundtrip` names a second, never-firing job so the command under test
/// touches nothing the assertion reads.
async fn await_completion_processed(
    registry: &ScheduledJobRegistry,
    handle: &SchedulerHandle,
    name: &str,
    roundtrip: &str,
) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if registry.job(name).and_then(|i| i.last_duration).is_some() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{name}: the driver processed no tick completion"));

    assert!(
        handle.pause(roundtrip).await,
        "the roundtrip target must be known to the driver"
    );
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
        schedule: ScheduleConfig::Interval(
            r2e_scheduler::PositiveDuration::from_millis(50).unwrap(),
        ),
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

// ── Cron with no upcoming occurrences ────────────────────────────────────────

#[r2e_core::test]
async fn cron_pinned_to_the_past_never_arms() {
    // A fully-pinned cron in the past (year 2000) yields no upcoming fire, so
    // `cron_next_instant` returns None at initial arming and the job is dormant.
    let counter = Arc::new(AtomicUsize::new(0));
    let registry = ScheduledJobRegistry::new();
    let cancel = CancellationToken::new();
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
    let driver = tokio::spawn(r2e_scheduler::jobs_driver(
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
    let cancel = CancellationToken::new();
    let (handle, commands) = SchedulerHandle::channel(cancel.clone());
    let task = counting_task("one_shot_cron", ScheduleConfig::Cron(expr), counter.clone());
    let jobs: Vec<_> = [
        Box::new(task) as Box<dyn ScheduledTask>,
        Box::new(quiet_task("roundtrip")),
    ]
    .into_iter()
    .map(|t| t.into_job())
    .collect();
    let driver = tokio::spawn(r2e_scheduler::jobs_driver(
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
    let driver = tokio::spawn(r2e_scheduler::jobs_driver(
        jobs,
        CancellationToken::new(),
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

    let cancel = CancellationToken::new();
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
    let driver = tokio::spawn(r2e_scheduler::jobs_driver(
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

// ── A tick FACTORY that panics ──────────────────────────────────────────────
//
// Tick bodies panic inside the pool, which contains them. Tick *construction*
// runs user code (`state.clone()`, the closure body up to its first await) on
// the driver's own stack: an unguarded panic there unwinds the driver past its
// drain, detaching the ticks already in flight and dropping the tracked future
// that keeps the bean graph alive. The panic is caught at the construction
// site, the broken job is disabled, and the driver keeps driving.

#[r2e_core::test]
async fn a_panicking_tick_factory_disables_its_job_without_killing_the_driver() {
    #[derive(Clone)]
    struct State {
        calls: Arc<AtomicUsize>,
        started: Arc<tokio::sync::Notify>,
        finished: Arc<std::sync::atomic::AtomicBool>,
    }

    let state = State {
        calls: Arc::new(AtomicUsize::new(0)),
        started: Arc::new(tokio::sync::Notify::new()),
        finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    let healthy_runs = Arc::new(AtomicUsize::new(0));

    let panicky = ScheduledTaskDef::new(
        "panicky_factory",
        ScheduleConfig::Interval(r2e_scheduler::PositiveDuration::from_millis(50).unwrap()),
        state.clone(),
        |s: State| {
            let nth = s.calls.fetch_add(1, Ordering::SeqCst);
            // Second construction blows up — synchronously, before any future
            // exists, while the first tick is still running.
            assert!(nth != 1, "tick factory exploded");
            async move {
                if nth == 0 {
                    s.started.notify_one();
                    tokio::time::sleep(Duration::from_millis(800)).await;
                    s.finished.store(true, Ordering::SeqCst);
                }
            }
        },
    )
    .with_overlap(OverlapPolicy::Concurrent);

    let healthy = counting_task(
        "healthy_neighbour",
        ScheduleConfig::Interval(r2e_scheduler::PositiveDuration::from_millis(50).unwrap()),
        healthy_runs.clone(),
    )
    .with_overlap(OverlapPolicy::Concurrent);

    let registry = ScheduledJobRegistry::new();
    let cancel = CancellationToken::new();
    let jobs: Vec<_> = [
        Box::new(panicky) as Box<dyn ScheduledTask>,
        Box::new(healthy) as Box<dyn ScheduledTask>,
    ]
    .into_iter()
    .map(|t| t.into_job())
    .collect();
    let driver = tokio::spawn(r2e_scheduler::jobs_driver(
        jobs,
        cancel.clone(),
        test_pool(),
        registry.clone(),
        SchedulerCommands::disconnected(),
    ));

    state.started.notified().await;
    // Long enough for several more fires: the panicking one and a handful of
    // healthy ones.
    tokio::time::sleep(Duration::from_millis(250)).await;

    assert!(
        healthy_runs.load(Ordering::SeqCst) >= 2,
        "the neighbouring job must keep firing after another job's factory panicked"
    );
    let info = registry.job("panicky_factory").expect("job info");
    assert!(
        info.paused && info.panic_count >= 1,
        "a factory panic must disable the job and be recorded: {info:?}"
    );

    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), driver)
        .await
        .expect("the driver must return after cancellation")
        .expect("the driver task must survive a panic in a tick factory");

    assert!(
        state.finished.load(Ordering::SeqCst),
        "the tick already in flight must still be drained"
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
        let cancel = CancellationToken::new();
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
            triggers.push(tokio::spawn(async move {
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
        let driver = tokio::spawn(r2e_scheduler::jobs_driver(
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

// ── Resume is the universal re-arm ──────────────────────────────────────────
//
// A `Skip` job is re-armed by its own tick's completion. When its tick factory
// panics there IS no tick, and the heap entry that fired it was already
// consumed: clearing `paused` alone would leave the job silent forever with a
// stale `next_run`. Resume puts it back on the clock.

#[r2e_core::test]
async fn resume_revives_a_factory_disabled_skip_job() {
    #[derive(Clone)]
    struct State {
        calls: Arc<AtomicUsize>,
        runs: Arc<AtomicUsize>,
    }

    let state = State {
        calls: Arc::new(AtomicUsize::new(0)),
        runs: Arc::new(AtomicUsize::new(0)),
    };
    let task = ScheduledTaskDef::new(
        "revivable",
        ScheduleConfig::Interval(r2e_scheduler::PositiveDuration::from_millis(50).unwrap()),
        state.clone(),
        |s: State| {
            let nth = s.calls.fetch_add(1, Ordering::SeqCst);
            assert!(nth != 1, "tick factory exploded");
            async move {
                s.runs.fetch_add(1, Ordering::SeqCst);
            }
        },
    ); // default OverlapPolicy::Skip

    let registry = ScheduledJobRegistry::new();
    let cancel = CancellationToken::new();
    let (handle, commands) = SchedulerHandle::channel(cancel.clone());
    let boxed: Box<dyn ScheduledTask> = Box::new(task);
    let jobs: Vec<_> = [boxed].into_iter().map(|t| t.into_job()).collect();
    let driver = tokio::spawn(r2e_scheduler::jobs_driver(
        jobs,
        cancel.clone(),
        test_pool(),
        registry.clone(),
        commands,
    ));

    // Fire #0 runs, fire #1 panics in the factory → disabled and off the heap.
    tokio::time::sleep(Duration::from_millis(250)).await;
    let disabled = registry.job("revivable").expect("job info");
    assert!(disabled.paused, "the factory panic must disable the job");
    assert!(
        disabled.next_run.is_none(),
        "a Skip job disabled by a factory panic holds no deadline: its heap entry \
         was consumed at the pop, so `next_run` must not advertise a fire that \
         nobody will ever deliver (got {:?})",
        disabled.next_run
    );
    let runs_while_disabled = state.runs.load(Ordering::SeqCst);
    assert_eq!(
        runs_while_disabled, 1,
        "a disabled Skip job must not fire again on its own"
    );

    assert!(handle.resume("revivable").await, "resume must apply");

    tokio::time::timeout(Duration::from_secs(3), async {
        while state.runs.load(Ordering::SeqCst) <= runs_while_disabled {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect(
        "resume must put a factory-disabled Skip job back on the clock: it has no heap \
         entry left, so clearing `paused` alone can never revive it",
    );

    // The body counter above increments INSIDE the pool job, so it proves the
    // tick ran, not that the driver processed its completion and re-armed.
    // Wait for the publication itself.
    let revived = await_next_run(&registry, "revivable").await;
    assert!(
        Some(revived) > disabled.next_run,
        "resume must publish a fresh next_run (was {:?}, now {revived:?})",
        disabled.next_run,
    );

    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), driver)
        .await
        .expect("driver must stop")
        .expect("driver task must not panic");
}

// ── …but resume must not arm a job that is already armed ───────────────────
//
// An ordinary pause keeps the cadence advancing on the heap. If resume pushed a
// second deadline there, the job would fire twice per period for the rest of
// the run.

#[r2e_core::test]
async fn pause_then_resume_does_not_double_arm() {
    // Each fire is timestamped: a second heap entry shows up as a *burst* —
    // two fires far closer together than the cadence — because both entries
    // share one cadence anchor and simply interleave afterwards.
    let fires: Arc<std::sync::Mutex<Vec<std::time::Instant>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    const PERIOD: Duration = Duration::from_millis(400);

    let task = ScheduledTaskDef::new(
        "steady",
        ScheduleConfig::Interval(r2e_scheduler::PositiveDuration::from_millis(400).unwrap()),
        fires.clone(),
        |f: Arc<std::sync::Mutex<Vec<std::time::Instant>>>| {
            f.lock().unwrap().push(std::time::Instant::now());
            async {}
        },
    )
    .with_overlap(OverlapPolicy::Concurrent);

    let registry = ScheduledJobRegistry::new();
    let cancel = CancellationToken::new();
    let (handle, commands) = SchedulerHandle::channel(cancel.clone());
    let boxed: Box<dyn ScheduledTask> = Box::new(task);
    let jobs: Vec<_> = [boxed].into_iter().map(|t| t.into_job()).collect();
    let driver = tokio::spawn(r2e_scheduler::jobs_driver(
        jobs,
        cancel.clone(),
        test_pool(),
        registry.clone(),
        commands,
    ));

    assert!(handle.pause("steady").await, "pause must apply");
    // Long enough for the paused deadline to pop and re-arm itself: the job is
    // armed again when the resume lands.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(handle.resume("steady").await, "resume must apply");

    tokio::time::sleep(Duration::from_millis(1500)).await;
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), driver)
        .await
        .expect("driver must stop")
        .expect("driver task must not panic");

    let fires = fires.lock().unwrap().clone();
    assert!(
        fires.len() >= 2,
        "the job must keep firing after resume (got {} fires)",
        fires.len()
    );
    let min_gap = fires
        .windows(2)
        .map(|w| w[1].duration_since(w[0]))
        .min()
        .expect("at least two fires");
    assert!(
        min_gap >= PERIOD * 5 / 8,
        "two fires {min_gap:?} apart at a {PERIOD:?} cadence: resume armed a second \
         deadline for a job that was still armed"
    );
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
    let cancel = CancellationToken::new();
    let (handle, commands) = SchedulerHandle::channel(cancel.clone());
    let jobs: Vec<_> = [
        Box::new(task) as Box<dyn ScheduledTask>,
        Box::new(quiet_task("roundtrip")),
    ]
    .into_iter()
    .map(|t| t.into_job())
    .collect();
    let driver = tokio::spawn(r2e_scheduler::jobs_driver(
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

// ── A closed pool is terminal on EVERY submission site ─────────────────────
//
// With no job due for an hour, the trigger-now submission is the only one that
// can observe the closed pool. If it just reported `false`, the driver would
// park indefinitely — holding the bean graph alive as a tracked task.

#[r2e_core::test]
async fn trigger_now_against_a_closed_pool_stops_the_driver() {
    let runs = Arc::new(AtomicUsize::new(0));
    let task = counting_task(
        "hourly",
        ScheduleConfig::IntervalWithDelay {
            interval: r2e_scheduler::PositiveDuration::from_secs(3600).unwrap(),
            initial_delay: Duration::from_secs(3600),
        },
        runs.clone(),
    );

    let pool = test_pool();
    let cancel = CancellationToken::new();
    let (handle, commands) = SchedulerHandle::channel(cancel.clone());
    let boxed: Box<dyn ScheduledTask> = Box::new(task);
    let jobs: Vec<_> = [boxed].into_iter().map(|t| t.into_job()).collect();
    let driver = tokio::spawn(r2e_scheduler::jobs_driver(
        jobs,
        cancel.clone(),
        pool.clone(),
        ScheduledJobRegistry::new(),
        commands,
    ));

    pool.shutdown();
    assert!(
        !handle.trigger_now("hourly").await,
        "a trigger against a closed pool must report false"
    );

    // Note: the token is deliberately NOT cancelled — the driver must stop on
    // its own, the way it does for a cadence submission against a closed pool.
    tokio::time::timeout(Duration::from_secs(5), driver)
        .await
        .expect(
            "a closed pool must be terminal at the trigger-now submission site too: with no \
             job due, nothing else would ever observe it and the driver would park forever",
        )
        .expect("driver task must not panic");
    assert_eq!(runs.load(Ordering::SeqCst), 0, "no tick can have run");
}

// ── The resume contract on the paths where the deadline SURVIVES ────────────
//
// `Concurrent` arms its next deadline *before* building the tick, so a factory
// panic disables a job that is still on the heap. The contract covers both
// shapes with one sentence: resume keeps an already-armed deadline and re-arms
// only a job that has none.

#[r2e_core::test]
async fn a_concurrent_factory_panic_keeps_the_deadline_it_already_armed() {
    #[derive(Clone)]
    struct State {
        calls: Arc<AtomicUsize>,
        runs: Arc<AtomicUsize>,
    }

    let state = State {
        calls: Arc::new(AtomicUsize::new(0)),
        runs: Arc::new(AtomicUsize::new(0)),
    };
    let task = ScheduledTaskDef::new(
        "kept",
        ScheduleConfig::Interval(r2e_scheduler::PositiveDuration::from_millis(5000).unwrap()),
        state.clone(),
        |s: State| {
            let nth = s.calls.fetch_add(1, Ordering::SeqCst);
            assert!(nth != 0, "tick factory exploded"); // only the first build panics
            async move {
                s.runs.fetch_add(1, Ordering::SeqCst);
            }
        },
    )
    .with_overlap(OverlapPolicy::Concurrent);

    let registry = ScheduledJobRegistry::new();
    let cancel = CancellationToken::new();
    let (handle, commands) = SchedulerHandle::channel(cancel.clone());
    let boxed: Box<dyn ScheduledTask> = Box::new(task);
    let jobs: Vec<_> = [boxed].into_iter().map(|t| t.into_job()).collect();
    let driver = tokio::spawn(r2e_scheduler::jobs_driver(
        jobs,
        cancel.clone(),
        test_pool(),
        registry.clone(),
        commands,
    ));

    // t≈0: a plain interval fires immediately. `Concurrent` arms t≈5000ms
    // BEFORE building the tick, so the factory panic disables a job that is
    // still on the heap — and the next pop is far enough out that no cadence
    // event can interleave with the assertions below.
    tokio::time::sleep(Duration::from_millis(3000)).await;
    // Poll for the deadline rather than sampling: every pop clears `next_run`
    // before re-arming, so a point read can land in that truthful transient.
    let kept_deadline = await_next_run(&registry, "kept").await;
    let disabled = registry.job("kept").expect("job info");
    assert!(disabled.paused, "the factory panic must disable the job");
    assert_eq!(
        state.runs.load(Ordering::SeqCst),
        0,
        "a paused job never fires, deadline or not"
    );

    // t≈3000ms: resume, ~2000ms short of the deadline the job still holds and
    // ~2000ms clear of it — 1.5s of scheduling noise in either direction cannot
    // flip the outcome below (kept ≈2000ms after the resume, re-armed-from-now
    // would be ≈5000ms).
    let resumed_at = std::time::Instant::now();
    assert!(
        handle.resume("kept").await,
        "resume must report true: the job will fire again"
    );
    assert_eq!(
        await_next_run(&registry, "kept").await,
        kept_deadline,
        "resume must keep the deadline the job already holds, not shift it"
    );

    // Margin arithmetic: the kept deadline fires ≈2000ms after the resume; the
    // failure mode (re-armed from now) fires ≈5000ms after it. The DISCRIMINANT
    // is the 3500ms assert below — so this timeout must never be the binding
    // constraint, or a re-armed-from-now fire would surface as an opaque
    // timeout instead of the assert quoting the real delay. 10s = 5000ms
    // (worst legitimate observation) + 5000ms of scheduling noise, comfortably
    // above the 3500ms threshold it must not pre-empt.
    tokio::time::timeout(Duration::from_secs(10), async {
        while state.runs.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the resumed job must fire");
    let waited = resumed_at.elapsed();
    assert!(
        waited < Duration::from_millis(3500),
        "the job fired {waited:?} after resume: it was re-armed from now (~5000ms) \
         instead of honouring the deadline it still held (~2000ms out)"
    );

    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), driver)
        .await
        .expect("driver must stop")
        .expect("driver task must not panic");
}

#[r2e_core::test]
async fn a_trigger_now_factory_panic_leaves_the_cadence_deadline_alone() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let task = ScheduledTaskDef::new(
        "oob_panic",
        // A plain interval fires immediately; the delay keeps the only cadence
        // fire far outside the test so the OOB tick is the sole factory call.
        ScheduleConfig::IntervalWithDelay {
            interval: r2e_scheduler::PositiveDuration::from_millis(60_000).unwrap(),
            initial_delay: Duration::from_secs(30),
        },
        (),
        move |()| {
            c.fetch_add(1, Ordering::SeqCst);
            panic!("tick factory exploded");
            #[allow(unreachable_code)]
            async {}
        },
    );

    let registry = ScheduledJobRegistry::new();
    let cancel = CancellationToken::new();
    let (handle, commands) = SchedulerHandle::channel(cancel.clone());
    let boxed: Box<dyn ScheduledTask> = Box::new(task);
    let jobs: Vec<_> = [boxed].into_iter().map(|t| t.into_job()).collect();
    let driver = tokio::spawn(r2e_scheduler::jobs_driver(
        jobs,
        cancel.clone(),
        test_pool(),
        registry.clone(),
        commands,
    ));

    tokio::time::sleep(Duration::from_millis(100)).await;
    let armed_deadline = await_next_run(&registry, "oob_panic").await;

    assert!(
        !handle.trigger_now("oob_panic").await,
        "a tick whose factory panicked did not run"
    );
    let disabled = registry.job("oob_panic").expect("job info");
    assert!(disabled.paused, "the factory panic must disable the job");
    assert_eq!(disabled.panic_count, 1, "the panic is accounted for");
    assert_eq!(
        disabled.next_run,
        Some(armed_deadline),
        "an out-of-band tick never touches the regular schedule: the cadence \
         deadline (and its telemetry) survives the panic"
    );

    assert!(
        handle.resume("oob_panic").await,
        "resume must report true: the job still holds a deadline"
    );
    assert_eq!(
        await_next_run(&registry, "oob_panic").await,
        armed_deadline,
        "resume keeps an armed deadline instead of re-arming from now"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1, "only the OOB tick was built");

    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), driver)
        .await
        .expect("driver must stop")
        .expect("driver task must not panic");
}

// ── …and the path where the schedule can never fire again ──────────────────

#[r2e_core::test]
async fn resume_reports_false_when_the_schedule_can_never_fire_again() {
    let counter = Arc::new(AtomicUsize::new(0));
    let registry = ScheduledJobRegistry::new();
    let cancel = CancellationToken::new();
    let (handle, commands) = SchedulerHandle::channel(cancel.clone());
    // Fully pinned to the year 2000: no upcoming occurrence, ever.
    let task = counting_task(
        "spent_cron",
        ScheduleConfig::Cron("0 0 0 1 1 * 2000".to_string()),
        counter.clone(),
    );
    let boxed: Box<dyn ScheduledTask> = Box::new(task);
    let jobs: Vec<_> = [boxed].into_iter().map(|t| t.into_job()).collect();
    let driver = tokio::spawn(r2e_scheduler::jobs_driver(
        jobs,
        cancel.clone(),
        test_pool(),
        registry.clone(),
        commands,
    ));

    // The job IS known to the driver — this is not the unknown-name `false`.
    // The reply doubles as the barrier for the `is_none()` below: it proves the
    // pre-loop arming pass ran, so a sleep-length race can no longer make
    // "nothing armed yet" look like "nothing armable".
    assert!(handle.pause("spent_cron").await, "the job is known");
    assert!(
        registry
            .job("spent_cron")
            .expect("job info")
            .next_run
            .is_none(),
        "a cron pinned to the past arms nothing"
    );

    assert!(
        !handle.resume("spent_cron").await,
        "resume promised a revival it cannot deliver: this schedule has no next \
         occurrence, so the job cannot fire again and the reply must be false"
    );
    let after = registry.job("spent_cron").expect("still listed");
    assert!(!after.paused, "the paused flag is still cleared");
    assert!(after.next_run.is_none(), "nothing was armed");
    assert_eq!(counter.load(Ordering::SeqCst), 0, "and nothing fired");

    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), driver)
        .await
        .expect("driver must stop")
        .expect("driver task must not panic");
}

// ── Armed by proxy: a `Skip` tick in flight is a PROMISE of a re-arm ────────
//
// While a Skip tick runs, its deadline is spent and the job is armed only by
// proxy: the completion will call `arm_next`. Whether that yields a fire
// depends on the schedule still having one — so `resume` probes it instead of
// answering from the bare fact that something is running.

#[derive(Clone)]
struct TickTrace {
    starts: Arc<AtomicUsize>,
    ends: Arc<AtomicUsize>,
    /// Completion gate. The tick body takes a permit before counting itself
    /// finished, so a GATED tick that has started provably cannot complete
    /// until the test hands one out. That is what makes the assertions below
    /// delay-proof: they claim things about the driver's `PendingRearm` state,
    /// and without the gate a slow test task lets the completion land first —
    /// the asserts would then pass through `Unarmed`/`Deadline` and the
    /// mutations they are supposed to kill would survive.
    gate: Arc<Semaphore>,
}

impl TickTrace {
    /// Every tick blocks at the end of its body until `release`.
    fn gated() -> Self {
        Self {
            starts: Arc::new(AtomicUsize::new(0)),
            ends: Arc::new(AtomicUsize::new(0)),
            gate: Arc::new(Semaphore::new(0)),
        }
    }

    /// Open the gate for good: every tick in flight, and every tick after it,
    /// may complete. Deliberately not a finite count — a test that hands out
    /// `n` permits while the cadence keeps firing can run out, and the `n+1`th
    /// tick then blocks forever inside the driver's shutdown drain, turning a
    /// timing hiccup into a hung test. `MAX_PERMITS >> 4` is unreachable in a
    /// test and leaves room for several calls.
    fn open(&self) {
        self.gate.add_permits(Semaphore::MAX_PERMITS >> 4);
    }

    async fn await_ends(&self, n: usize, what: &str) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while self.ends.load(Ordering::SeqCst) < n {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{what}"));
    }
}

fn tracing_task(name: &str, schedule: ScheduleConfig, trace: TickTrace) -> ScheduledTaskDef<TickTrace> {
    ScheduledTaskDef::new(name, schedule, trace, |t: TickTrace| async move {
        t.starts.fetch_add(1, Ordering::SeqCst);
        t.gate
            .acquire()
            .await
            .expect("the completion gate is never closed")
            .forget();
        t.ends.fetch_add(1, Ordering::SeqCst);
    })
}

/// A job that never fires on its own: a roundtrip target for driver-serialized
/// observations (its `pause` reply proves the driver finished the due-loop
/// iteration that submitted another job's tick).
fn quiet_task(name: &str) -> ScheduledTaskDef<()> {
    ScheduledTaskDef::new(
        name,
        ScheduleConfig::IntervalWithDelay {
            interval: r2e_scheduler::PositiveDuration::from_secs(3600).unwrap(),
            initial_delay: Duration::from_secs(3600),
        },
        (),
        |()| async {},
    )
}

async fn await_starts(trace: &TickTrace, n: usize, what: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while trace.starts.load(Ordering::SeqCst) < n {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what}"));
}

#[r2e_core::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_reports_false_when_the_in_flight_tick_is_the_last_occurrence() {
    // A cron pinned to a single wall-clock second ~2s out: the tick that fires
    // it is the schedule's FINAL occurrence, and it runs long enough to be
    // resumed mid-flight.
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

    // Gated: the tick holds the driver in `PendingRearm` until this test says
    // otherwise, so the assertions below cannot be satisfied by a completion
    // that sneaked in first.
    let trace = TickTrace::gated();
    let task = tracing_task("last_slot", ScheduleConfig::Cron(expr), trace.clone());

    let registry = ScheduledJobRegistry::new();
    let cancel = CancellationToken::new();
    let (handle, commands) = SchedulerHandle::channel(cancel.clone());
    let jobs: Vec<_> = [
        Box::new(task) as Box<dyn ScheduledTask>,
        // Roundtrip target for the post-completion barrier: a command on THIS
        // job touches nothing the assertions below read.
        Box::new(quiet_task("roundtrip")),
    ]
    .into_iter()
    .map(|t| t.into_job())
    .collect();
    let driver = tokio::spawn(r2e_scheduler::jobs_driver(
        jobs,
        cancel.clone(),
        test_pool(),
        registry.clone(),
        commands,
    ));

    await_starts(&trace, 1, "the single cron occurrence must fire").await;

    // Mid-tick, gate CLOSED: the tick cannot finish, so the driver is provably
    // in `PendingRearm` — the job holds no deadline, only the promise of a
    // re-arm. (The tick started ⟹ the submission ran; the driver has no await
    // inside the due loop, so a command reply can only come after that
    // iteration finished; and the completion arm cannot run at all.)
    assert!(handle.pause("last_slot").await, "the job is known");
    assert!(
        !handle.resume("last_slot").await,
        "resume promised a revival it cannot deliver: the tick in flight is the \
         schedule's last occurrence, so its completion re-arms nothing"
    );

    // Completion confirms it: unarmed, nothing published, nothing more fires.
    // `ends` is incremented inside the pool job, so it proves the body finished,
    // not that the driver re-armed; and a sleep here would be worse than
    // useless, since a deadline wrongly published by that re-arm would be wiped
    // by the exit clear-all at `cancel()` below — the assert would then pass
    // BECAUSE of the bug. Wait for the driver's own completion bookkeeping.
    trace.open();
    trace.await_ends(1, "the tick must finish once released").await;
    await_completion_processed(&registry, &handle, "last_slot", "roundtrip").await;

    let info = registry.job("last_slot").expect("still listed");
    assert!(
        info.next_run.is_none(),
        "a spent cron publishes no deadline (got {:?})",
        info.next_run
    );
    assert_eq!(
        trace.starts.load(Ordering::SeqCst),
        1,
        "the schedule had exactly one occurrence"
    );

    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), driver)
        .await
        .expect("driver must stop")
        .expect("driver task must not panic");
}

#[r2e_core::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_reports_true_while_a_healthy_skip_tick_is_in_flight() {
    // Same shape, live schedule: the completion WILL re-arm, so the probe finds
    // a next fire and the reply is true — and the job really does fire again.
    // Gated for the same reason as the last-occurrence test: without it a
    // delayed test task lets the tick complete first and `resume` answers from
    // `Deadline`, which is not the branch under test.
    let trace = TickTrace::gated();
    let task = tracing_task(
        "healthy",
        ScheduleConfig::Interval(r2e_scheduler::PositiveDuration::from_millis(400).unwrap()),
        trace.clone(),
    ); // default OverlapPolicy::Skip

    let registry = ScheduledJobRegistry::new();
    let cancel = CancellationToken::new();
    let (handle, commands) = SchedulerHandle::channel(cancel.clone());
    let boxed: Box<dyn ScheduledTask> = Box::new(task);
    let jobs: Vec<_> = [boxed].into_iter().map(|t| t.into_job()).collect();
    let driver = tokio::spawn(r2e_scheduler::jobs_driver(
        jobs,
        cancel.clone(),
        test_pool(),
        registry.clone(),
        commands,
    ));

    await_starts(&trace, 1, "the first tick must start").await;
    // Gate CLOSED: the driver is in `PendingRearm` when it answers.
    assert!(handle.pause("healthy").await, "the job is known");
    assert!(
        handle.resume("healthy").await,
        "the in-flight tick will re-arm a live schedule: resume must report true"
    );

    // Release enough permits for the first tick and the ones that follow it.
    trace.open();
    await_starts(&trace, 2, "the resumed job must fire again after its tick").await;

    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), driver)
        .await
        .expect("driver must stop")
        .expect("driver task must not panic");
}

#[r2e_core::test(flavor = "multi_thread", worker_threads = 2)]
async fn next_run_is_none_while_a_skip_tick_holds_the_job_off_the_clock() {
    let trace = TickTrace::gated();
    let task = tracing_task(
        "long_tick",
        ScheduleConfig::Interval(r2e_scheduler::PositiveDuration::from_millis(300).unwrap()),
        trace.clone(),
    ); // default OverlapPolicy::Skip

    let registry = ScheduledJobRegistry::new();
    let cancel = CancellationToken::new();
    let (handle, commands) = SchedulerHandle::channel(cancel.clone());
    let jobs: Vec<_> = [
        Box::new(task) as Box<dyn ScheduledTask>,
        Box::new(quiet_task("roundtrip")),
    ]
    .into_iter()
    .map(|t| t.into_job())
    .collect();
    let driver = tokio::spawn(r2e_scheduler::jobs_driver(
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
    let cancel = CancellationToken::new();
    let pool = test_pool();
    pool.shutdown(); // closed before the driver ever pops
    let (_handle, commands) = SchedulerHandle::channel(cancel.clone());
    let boxed: Box<dyn ScheduledTask> = Box::new(task);
    let jobs: Vec<_> = [boxed].into_iter().map(|t| t.into_job()).collect();
    let driver = tokio::spawn(r2e_scheduler::jobs_driver(
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
    let cancel = CancellationToken::new();
    let (_handle, commands) = SchedulerHandle::channel(cancel.clone());
    let boxed: Box<dyn ScheduledTask> = Box::new(task);
    let jobs: Vec<_> = [boxed].into_iter().map(|t| t.into_job()).collect();
    let driver = tokio::spawn(r2e_scheduler::jobs_driver(
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
