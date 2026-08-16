use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, Timelike, Utc};
use r2e_scheduler::{
    OverlapPolicy, ScheduleConfig, ScheduledJobRegistry, ScheduledTask, ScheduledTaskDef,
    SchedulerHandle,
};
use tokio_util::sync::CancellationToken;

use crate::support::{
    await_completion_processed, await_next_run, await_starts, counting_task, quiet_task, test_pool,
    tracing_task, TickTrace,
};

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
    let driver = r2e_core::rt::spawn(r2e_scheduler::jobs_driver(
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
    let driver = r2e_core::rt::spawn(r2e_scheduler::jobs_driver(
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
    let driver = r2e_core::rt::spawn(r2e_scheduler::jobs_driver(
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
    let driver = r2e_core::rt::spawn(r2e_scheduler::jobs_driver(
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
    let driver = r2e_core::rt::spawn(r2e_scheduler::jobs_driver(
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
    let driver = r2e_core::rt::spawn(r2e_scheduler::jobs_driver(
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
