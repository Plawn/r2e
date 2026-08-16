use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use r2e_executor::{ExecutorConfig, PoolExecutor};
use r2e_scheduler::{
    start_jobs, ScheduleConfig, ScheduledJobRegistry, ScheduledTask, ScheduledTaskDef,
    SchedulerCommands, SchedulerHandle,
};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

pub(crate) fn test_pool() -> PoolExecutor {
    PoolExecutor::new(ExecutorConfig::default())
}

pub(crate) fn counting_task(
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
pub(crate) async fn await_next_run(registry: &ScheduledJobRegistry, name: &str) -> chrono::DateTime<Utc> {
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
pub(crate) async fn await_completion_processed(
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

pub(crate) fn start_one(
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

#[derive(Clone)]
pub(crate) struct TickTrace {
    pub(crate) starts: Arc<AtomicUsize>,
    pub(crate) ends: Arc<AtomicUsize>,
    /// Completion gate. The tick body takes a permit before counting itself
    /// finished, so a GATED tick that has started provably cannot complete
    /// until the test hands one out. That is what makes the assertions below
    /// delay-proof: they claim things about the driver's `PendingRearm` state,
    /// and without the gate a slow test task lets the completion land first —
    /// the asserts would then pass through `Unarmed`/`Deadline` and the
    /// mutations they are supposed to kill would survive.
    pub(crate) gate: Arc<Semaphore>,
}

impl TickTrace {
    /// Every tick blocks at the end of its body until `release`.
    pub(crate) fn gated() -> Self {
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
    pub(crate) fn open(&self) {
        self.gate.add_permits(Semaphore::MAX_PERMITS >> 4);
    }

    pub(crate) async fn await_ends(&self, n: usize, what: &str) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while self.ends.load(Ordering::SeqCst) < n {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{what}"));
    }
}

pub(crate) fn tracing_task(name: &str, schedule: ScheduleConfig, trace: TickTrace) -> ScheduledTaskDef<TickTrace> {
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
pub(crate) fn quiet_task(name: &str) -> ScheduledTaskDef<()> {
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

pub(crate) async fn await_starts(trace: &TickTrace, n: usize, what: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while trace.starts.load(Ordering::SeqCst) < n {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what}"));
}
