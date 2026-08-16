//! The single-driver scheduler runtime.
//!
//! Instead of spawning one Tokio task per schedule, the scheduler spawns
//! exactly ONE driver task ([`jobs_driver`]) that owns every schedule. The
//! driver keeps a min-heap of next-fire deadlines; when the earliest deadline
//! is reached it submits the due tick bodies to the shared
//! [`PoolExecutor`](r2e_executor::PoolExecutor) and tracks the resulting
//! handles in a [`FuturesUnordered`].
//!
//! Re-arming depends on each job's [`OverlapPolicy`]:
//! - [`Skip`](OverlapPolicy::Skip): a job is re-armed only when its own tick
//!   completes, so per-job ticks never overlap while different jobs still run
//!   concurrently.
//! - [`Concurrent`](OverlapPolicy::Concurrent): a job is re-armed at *fire*
//!   time (the next deadline is pushed back before the tick is even submitted),
//!   so a slow tick never holds back the following one — ticks may overlap.
//!
//! The driver also accepts runtime [`Command`]s (pause / resume / trigger-now)
//! and keeps the [`ScheduledJobRegistry`](crate::ScheduledJobRegistry) stats
//! current (run count, last/next run, last duration, panic count, paused flag).

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use r2e_core::rt::JoinError;
use r2e_executor::PoolExecutor;

use crate::types::{OverlapPolicy, ScheduleConfig, ScheduledJob, SkipFn};
use crate::ScheduledJobRegistry;

/// A runtime control command delivered to the driver via [`SchedulerHandle`].
///
/// Each command carries a oneshot reply channel; the boolean answer reports
/// whether the command applied. `false` covers more than an undelivered
/// command: an unknown job name, a driver that is shutting down (every command
/// dequeued after cancellation is refused), a `TriggerNow` on a `Skip` job with
/// a tick already in flight, on a closed executor pool, or whose tick factory
/// panicked, and a `Resume` of a schedule that can never fire again. The
/// per-command contract is documented on [`SchedulerHandle`], which is the
/// authority; keep the two in step.
pub(crate) enum Command {
    /// Stop firing a job on its schedule (its cadence still advances silently).
    Pause {
        name: String,
        reply: oneshot::Sender<bool>,
    },
    /// Resume a previously paused job.
    Resume {
        name: String,
        reply: oneshot::Sender<bool>,
    },
    /// Fire a job once, immediately, out of band (even when paused).
    TriggerNow {
        name: String,
        reply: oneshot::Sender<bool>,
    },
}

/// The receiving end of the runtime command channel, handed to [`start_jobs`].
///
/// Created by the [`Scheduler`](crate::Scheduler) plugin at install time and
/// threaded into the driver. Tests and direct `start_jobs` callers that don't
/// exercise runtime control use [`disconnected`](Self::disconnected), which
/// leaves the driver's command branch permanently inert.
pub struct SchedulerCommands {
    rx: Option<mpsc::Receiver<Command>>,
}

impl SchedulerCommands {
    /// Wrap a live command receiver.
    pub(crate) fn new(rx: mpsc::Receiver<Command>) -> Self {
        Self { rx: Some(rx) }
    }

    /// A handle that never delivers a command — the driver's command branch
    /// stays parked. For tests and direct `start_jobs` calls with no controller.
    pub fn disconnected() -> Self {
        Self { rx: None }
    }
}

/// Build the scheduler driver future for `jobs` — ONE future owning all
/// schedules: a min-heap of next-fire deadlines drives when each job's tick
/// body is submitted to `executor`.
///
/// This is the form the [`Scheduler`](crate::Scheduler) plugin uses: it hands
/// the future to `ServeContext::track`, which spawns it as a *tracked* task —
/// one that owns a clone of the bean graph while it runs and is drained (and
/// cancelled) by the framework on every exit path, including an aborted boot.
/// Spawning it any other way (see [`start_jobs`]) detaches it from both.
///
/// `registry` receives live stats updates (pass a fresh
/// [`ScheduledJobRegistry`] if you don't care). `commands` carries runtime
/// control; use [`SchedulerCommands::disconnected`] when none is wired.
///
/// Ticks run on the pool (not inline), so a panicking tick is contained in its
/// pool job and the driver keeps ticking. When the pool rejects a submission
/// (it has shut down), the driver stops — nothing can run anymore. On
/// cancellation the driver stops arming new ticks and then waits out the ones
/// already in flight before completing, so joining this future is enough to
/// know no scheduled work is still touching the app.
// Deliberately not an `async fn`: `Send + 'static` is part of the contract
// here — `ServeContext::track` requires it, and stating it in the signature
// makes "this future is trackable" a compile error to break, instead of
// something inferred at each call site.
#[allow(clippy::manual_async_fn)]
pub fn jobs_driver(
    jobs: Vec<ScheduledJob>,
    cancel: CancellationToken,
    executor: PoolExecutor,
    registry: ScheduledJobRegistry,
    commands: SchedulerCommands,
) -> impl Future<Output = ()> + Send + 'static {
    async move {
        run_driver(jobs, cancel, executor, registry, commands).await;
    }
}

/// Spawn [`jobs_driver`] as a detached task.
///
/// Convenience for standalone drivers and tests: the caller owns the lifetime
/// through `cancel` and nothing else. The task is **not** tracked by the app —
/// it does not keep the bean graph alive and the framework will not drain it on
/// shutdown or on an aborted boot. Inside an R2E app, prefer building the
/// future with [`jobs_driver`] and handing it to `ServeContext::track` (what
/// the [`Scheduler`](crate::Scheduler) plugin does).
pub fn start_jobs(
    jobs: Vec<ScheduledJob>,
    cancel: CancellationToken,
    executor: PoolExecutor,
    registry: ScheduledJobRegistry,
    commands: SchedulerCommands,
) {
    r2e_core::rt::spawn(jobs_driver(jobs, cancel, executor, registry, commands));
}

/// How a job computes its next fire time when it is re-armed.
///
/// `Clone` so `Resume` can PROBE the next fire without consuming it:
/// [`compute_next`] advances the interval anchor in place, and a probe must
/// leave the job exactly as it found it.
#[derive(Clone)]
enum Rearm {
    /// Fixed cadence with skip, anchored at the job's initial arming: `deadline`
    /// tracks the last scheduled fire time; the next fire is the smallest
    /// `deadline + k*period` strictly greater than "now" (reproduces tokio's
    /// [`MissedTickBehavior::Skip`](r2e_core::rt::MissedTickBehavior::Skip)).
    Interval { period: Duration, deadline: Instant },
    /// Cron schedule, parsed once at arming.
    Cron(cron::Schedule),
}

/// Per-job state retained by the driver.
struct JobRuntime {
    name: String,
    run: Box<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>,
    /// Optional skip predicate evaluated at the start of every tick.
    skip: Option<SkipFn>,
    rearm: Rearm,
    overlap: OverlapPolicy,
    /// Paused jobs advance their cadence but never submit a scheduled tick.
    paused: bool,
    /// Number of ticks of this job currently running on the pool.
    in_flight: usize,
    /// How (and whether) a future fire is already accounted for.
    arm: ArmState,
}

/// Whether a job has a future fire accounted for — and, crucially, whether that
/// account is a fact or a promise.
///
/// This is what makes `Resume` a safe universal re-arm: it may push a deadline
/// only for a job in [`ArmState::Unarmed`]. Without the distinction, resuming a
/// job that was merely paused (its cadence keeps advancing on the heap) would
/// add a SECOND entry and the job would fire twice per period, for good.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ArmState {
    /// No deadline, and nothing running that would produce one. `Resume` may
    /// re-arm from now; `ScheduledJobInfo::next_run` reads `None`.
    Unarmed,
    /// A live heap entry: a fact. The job WILL fire at that instant unless it
    /// is paused, and `next_run` publishes it.
    Deadline,
    /// Armed by proxy: a `Skip` tick is in flight and its completion will call
    /// `arm_next`. A promise, not a fact — the re-arm yields a deadline only if
    /// the schedule still has one at completion time, so the job's deadline is
    /// consumed and `next_run` reads `None` for the duration of the tick.
    PendingRearm,
}

/// In-flight tick result: `(job index, re-arm on completion, wall duration,
/// skipped by predicate, join result)`.
type InFlight = FuturesUnordered<
    Pin<Box<dyn Future<Output = (usize, bool, Duration, bool, Result<(), JoinError>)> + Send>>,
>;

/// The next upcoming cron fire time as a tokio [`Instant`], or `None` if the
/// schedule has no further executions.
fn cron_next_instant(schedule: &cron::Schedule) -> Option<Instant> {
    let now_utc = Utc::now();
    let next = schedule.upcoming(Utc).next()?;
    let until = (next - now_utc).to_std().unwrap_or(Duration::ZERO);
    // `checked_add`: an occurrence far enough out to leave the monotonic
    // clock's range is treated as "no next occurrence" rather than a panic in
    // the driver loop.
    let Some(at) = Instant::now().checked_add(until) else {
        tracing::warn!(
            occurrence = %next,
            "Cron occurrence is beyond the monotonic clock's range; schedule exhausted"
        );
        return None;
    };
    Some(at)
}

/// Project a tokio [`Instant`] onto wall-clock time for user-facing stats.
fn instant_to_datetime(t: Instant) -> DateTime<Utc> {
    let now_inst = Instant::now();
    let now_utc = Utc::now();
    // `checked_add_signed`/`checked_sub_signed`: chrono's `+`/`-` PANIC when the
    // result leaves `DateTime<Utc>`'s range, and these projections are fed by
    // user-configured periods. A deadline that cannot be represented as a date
    // is reported as "now" — a stats artefact, never an unwind.
    if t >= now_inst {
        let d = chrono::Duration::from_std(t - now_inst).unwrap_or_default();
        now_utc.checked_add_signed(d).unwrap_or(now_utc)
    } else {
        let d = chrono::Duration::from_std(now_inst - t).unwrap_or_default();
        now_utc.checked_sub_signed(d).unwrap_or(now_utc)
    }
}

/// Advance a job's `rearm` state and return its next fire instant (`None` for an
/// exhausted cron schedule).
fn compute_next(rearm: &mut Rearm, now: Instant, name: &str) -> Option<Instant> {
    match rearm {
        Rearm::Interval { period, deadline } => {
            // `checked_add`, not `+`: `PositiveDuration` bounds the period from
            // below (non-zero) but not from above, so a configured period near
            // `u64::MAX` seconds would panic on overflow — inside the driver
            // loop, unwinding past the drain. An unrepresentable next fire is
            // treated like a spent cron: the job is simply never armed again.
            let Some(mut next) = deadline.checked_add(*period) else {
                tracing::warn!(
                    task = %name,
                    "Interval overflows the monotonic clock; schedule exhausted"
                );
                return None;
            };
            while next <= now {
                let Some(n) = next.checked_add(*period) else {
                    tracing::warn!(
                        task = %name,
                        "Interval overflows the monotonic clock; schedule exhausted"
                    );
                    return None;
                };
                next = n;
            }
            *deadline = next;
            Some(next)
        }
        Rearm::Cron(schedule) => match cron_next_instant(schedule) {
            Some(t) => Some(t),
            None => {
                tracing::warn!(task = %name, "No more upcoming cron executions");
                None
            }
        },
    }
}

/// Sleep until `deadline`, or park forever when the heap is empty.
async fn wait_until(deadline: Option<Instant>) {
    match deadline {
        Some(t) => tokio::time::sleep_until(t).await,
        None => std::future::pending::<()>().await,
    }
}

/// Await the next command, or park forever when the channel is absent/closed.
async fn next_command(rx: &mut Option<mpsc::Receiver<Command>>) -> Option<Command> {
    match rx.as_mut() {
        Some(r) => r.recv().await,
        None => std::future::pending().await,
    }
}

/// A tick body ready to be submitted, plus the flag its skip predicate sets
/// when it suppresses the body.
type BuiltTick = (
    Pin<Box<dyn Future<Output = ()> + Send>>,
    Option<Arc<AtomicBool>>,
);

/// Outcome of one submission attempt.
///
/// Three cases, because two of them are *not* "stop the driver": only a closed
/// pool is terminal.
enum Submission {
    /// The tick is in flight and its completion is in `in_flight`.
    Submitted,
    /// The pool refuses new work — nothing can run anymore, so the driver stops
    /// (through the common drain).
    PoolClosed,
    /// The job's tick factory panicked **synchronously** (see [`build_tick`]).
    /// The job is disabled; the driver keeps running.
    FactoryPanicked,
}

/// Build the future for one tick — the part that runs **user code inline**.
///
/// `(runtime.run)()` and `(runtime.skip)()` are the closures built in
/// `ScheduledTaskDef::into_job`; calling them clones the task state and runs
/// whatever the user's closure body does before it returns its future. That all
/// happens on the driver's own stack, which is why the caller wraps this in
/// `catch_unwind`.
///
/// A job with a skip predicate evaluates it inside the pool job, before the
/// body: a `true` verdict suppresses the body and is recorded as a skip
/// (`skip_count`) instead of a run — `last_run`/`run_count` then move inside
/// the tick so they only reflect ticks whose body actually started.
fn build_tick(runtime: &JobRuntime, registry: &ScheduledJobRegistry) -> BuiltTick {
    let run_fut = (runtime.run)();
    match &runtime.skip {
        None => (run_fut, None),
        Some(skip) => {
            let skip_fut = skip();
            let flag = Arc::new(AtomicBool::new(false));
            let flag_in_tick = Arc::clone(&flag);
            let registry = registry.clone();
            let name = runtime.name.clone();
            let fut = Box::pin(async move {
                if skip_fut.await {
                    flag_in_tick.store(true, Ordering::Relaxed);
                    tracing::debug!(task = %name, "Scheduled tick skipped by skip predicate");
                    registry.update_job(&name, |i| i.skip_count += 1);
                } else {
                    registry.update_job(&name, |i| {
                        i.last_run = Some(Utc::now());
                        i.run_count += 1;
                    });
                    run_fut.await;
                }
            });
            (fut as Pin<Box<dyn Future<Output = ()> + Send>>, Some(flag))
        }
    }
}

/// Best-effort human text for a caught panic payload.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> &str {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        s
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.as_str()
    } else {
        "<non-string panic payload>"
    }
}

/// Submit one tick of job `idx` to the pool. `rearm` records whether the tick's
/// completion should re-arm the job (true only for `Skip` scheduled ticks).
///
/// Tick *bodies* panic inside the pool, where the executor contains them and the
/// driver only sees a `JoinError`. Tick *construction* is different: it runs
/// user code on the driver's stack, so an unguarded panic there would unwind the
/// driver itself — past the drain below, dropping (detaching) every `JobHandle`
/// still in `in_flight` and killing the tracked future that keeps the bean graph
/// alive. It is caught here instead.
fn submit_tick(
    idx: usize,
    rearm: bool,
    runtimes: &mut [JobRuntime],
    executor: &PoolExecutor,
    in_flight: &mut InFlight,
    registry: &ScheduledJobRegistry,
) -> Submission {
    let start = Instant::now();
    // `AssertUnwindSafe` is sound here for the same reason as the plugin
    // shutdown cell: the closure borrows the job runtime and the registry
    // *immutably* and returns owned values. Nothing the driver relies on is
    // mutated inside the boundary, so a panic cannot leave the heap, the
    // in-flight set or the per-job counters half-updated.
    let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        build_tick(&runtimes[idx], registry)
    }));
    let (fut, skipped_flag) = match built {
        Ok(built) => built,
        Err(payload) => {
            tracing::error!(
                task = %runtimes[idx].name,
                panic = %panic_text(payload.as_ref()),
                "Scheduled tick factory panicked; disabling the job"
            );
            return Submission::FactoryPanicked;
        }
    };
    match executor.submit(fut) {
        Ok(handle) => {
            runtimes[idx].in_flight += 1;
            // Skip-predicated jobs record last_run/run_count inside the tick
            // (only when the body actually runs); plain jobs record at submit.
            if skipped_flag.is_none() {
                registry.update_job(&runtimes[idx].name, |i| {
                    i.last_run = Some(instant_to_datetime(start));
                    i.run_count += 1;
                });
            }
            in_flight.push(Box::pin(async move {
                let res = handle.await;
                let skipped = skipped_flag.is_some_and(|f| f.load(Ordering::Relaxed));
                (idx, rearm, start.elapsed(), skipped, res)
            }));
            Submission::Submitted
        }
        Err(_) => Submission::PoolClosed,
    }
}

/// Disable a job whose tick factory panicked.
///
/// **Disable, not retry.** A factory panic is a property of the job's own
/// construction — a `Clone` that panics, a closure unwrapping a poisoned lock, a
/// missing resource — not of the tick's workload, so it reproduces on every
/// fire. Retrying turns one bug into a panic loop at the schedule's own cadence,
/// each iteration unwinding through the driver's stack rather than a contained
/// pool job. The job is paused instead (its `panic_count` records why), which
/// keeps the failure visible in `ScheduledJobInfo` and leaves the operator an
/// explicit way back in: `SchedulerHandle::resume` re-enables it.
///
/// Pausing is all this does — it deliberately does NOT disarm. Whether a
/// deadline survives the panic depends on where the panic happened, and the
/// resume contract covers both cases without the driver having to care:
/// `Concurrent` (and trigger-now) panic *after* the next cadence entry is on
/// the heap, so the job stays armed and `resume` honours that deadline; `Skip`
/// panics with its entry already consumed and nothing to re-arm it, so the job
/// ends unarmed and `resume` re-arms it from now.
fn disable_after_factory_panic(
    idx: usize,
    runtimes: &mut [JobRuntime],
    registry: &ScheduledJobRegistry,
) {
    runtimes[idx].paused = true;
    // Telemetry must not advertise a fire that cannot happen: a `Skip` panic is
    // disabled with its heap entry already consumed (unarmed), so the published
    // `next_run` is a deadline nobody holds. A `Concurrent`/trigger-now panic
    // leaves the cadence entry standing (armed) and keeps publishing it —
    // exactly the deadline `resume` will honour.
    let unarmed = runtimes[idx].arm == ArmState::Unarmed;
    let name = runtimes[idx].name.clone();
    registry.update_job(&name, |i| {
        i.paused = true;
        i.panic_count += 1;
        if unarmed {
            i.next_run = None;
        }
    });
}

/// Push a job's next deadline onto the heap and mirror it into the registry.
fn arm_next(
    idx: usize,
    runtimes: &mut [JobRuntime],
    heap: &mut BinaryHeap<Reverse<(Instant, usize)>>,
    registry: &ScheduledJobRegistry,
    now: Instant,
) {
    let next = compute_next(&mut runtimes[idx].rearm, now, &runtimes[idx].name);
    if let Some(t) = next {
        heap.push(Reverse((t, idx)));
    }
    // Exhausted (spent cron, unrepresentable next fire) leaves the job UNarmed,
    // which is precisely the state `Resume` is allowed to repair.
    runtimes[idx].arm = if next.is_some() {
        ArmState::Deadline
    } else {
        ArmState::Unarmed
    };
    registry.update_job(&runtimes[idx].name, |i| {
        i.next_run = next.map(instant_to_datetime);
    });
}

/// Put a job that has no live deadline back on the clock, starting from now.
///
/// Used by `Resume`. For an interval schedule the cadence anchor is reset to
/// "now" rather than replayed from the stale deadline: the job was off the
/// clock, so catching up thousands of missed fires would be noise (and, with a
/// short period and a long outage, a long synchronous loop inside the driver).
/// A cron schedule simply takes its next upcoming occurrence.
fn rearm_from_now(
    idx: usize,
    runtimes: &mut [JobRuntime],
    heap: &mut BinaryHeap<Reverse<(Instant, usize)>>,
    registry: &ScheduledJobRegistry,
) {
    let now = Instant::now();
    if let Rearm::Interval { deadline, .. } = &mut runtimes[idx].rearm {
        *deadline = now;
    }
    arm_next(idx, runtimes, heap, registry, now);
}

/// Set/clear a job's paused flag. Returns `false` for an unknown job.
fn set_paused(
    name: &str,
    paused: bool,
    runtimes: &mut [JobRuntime],
    registry: &ScheduledJobRegistry,
) -> bool {
    match runtimes.iter_mut().find(|j| j.name == name) {
        Some(job) => {
            job.paused = paused;
            registry.update_job(name, |i| i.paused = paused);
            true
        }
        None => false,
    }
}

async fn run_driver(
    jobs: Vec<ScheduledJob>,
    cancel: CancellationToken,
    executor: PoolExecutor,
    registry: ScheduledJobRegistry,
    commands: SchedulerCommands,
) {
    let now = Instant::now();
    let mut runtimes: Vec<JobRuntime> = Vec::with_capacity(jobs.len());
    let mut heap: BinaryHeap<Reverse<(Instant, usize)>> = BinaryHeap::new();
    let mut command_rx = commands.rx;

    // Initial arming.
    for job in jobs {
        let idx = runtimes.len();
        let (rearm, first): (Rearm, Option<Instant>) = match &job.schedule {
            // Fires immediately, matching tokio interval's immediate first tick.
            ScheduleConfig::Interval(period) => (
                Rearm::Interval {
                    period: period.get(),
                    deadline: now,
                },
                Some(now),
            ),
            ScheduleConfig::IntervalWithDelay {
                interval,
                initial_delay,
            } => {
                // `initial_delay` is an unrestricted `Duration` (unlike the
                // period, which is a `PositiveDuration`), so `now + delay` can
                // overflow — here, during initial arming, OUTSIDE the tick
                // factory's `catch_unwind` and before the drain exists. An
                // unrepresentable first fire arms nothing: the job is exhausted
                // from birth, exactly like a spent cron, and stays listed with
                // `next_run: None` until someone `resume`s it.
                match now.checked_add(*initial_delay) {
                    Some(first) => (
                        Rearm::Interval {
                            period: interval.get(),
                            deadline: first,
                        },
                        Some(first),
                    ),
                    None => {
                        tracing::warn!(
                            task = %job.name,
                            "Initial delay overflows the monotonic clock; job never armed"
                        );
                        (
                            Rearm::Interval {
                                period: interval.get(),
                                deadline: now,
                            },
                            None,
                        )
                    }
                }
            }
            ScheduleConfig::Cron(expr) => match expr.parse::<cron::Schedule>() {
                Ok(schedule) => {
                    let first = cron_next_instant(&schedule);
                    (Rearm::Cron(schedule), first)
                }
                Err(e) => {
                    // Retire the job: log and skip without registering it.
                    tracing::error!(task = %job.name, error = %e, "Invalid cron expression");
                    continue;
                }
            },
        };

        // Ensure the registry has an entry (idempotent: the plugin pre-registers
        // metadata; direct `start_jobs` callers get an entry auto-created here).
        registry.upsert(&job.name, &crate::format_schedule(&job.schedule));
        if let Some(t) = first {
            heap.push(Reverse((t, idx)));
            registry.update_job(&job.name, |i| i.next_run = Some(instant_to_datetime(t)));
        }
        runtimes.push(JobRuntime {
            name: job.name,
            run: job.run,
            skip: job.skip,
            rearm,
            overlap: job.overlap,
            paused: false,
            in_flight: 0,
            arm: if first.is_some() {
                ArmState::Deadline
            } else {
                ArmState::Unarmed
            },
        });
    }

    let count = runtimes.len();
    tracing::info!(count, "Scheduler driver started");

    let mut in_flight: InFlight = FuturesUnordered::new();

    // Labelled: the executor-rejection arms live inside the inner `while heap`
    // loop and must leave the DRIVER, not just that loop — and they must leave
    // it through the drain below like every other exit. There is exactly one
    // way out of this loop, and it is a `break 'driver`.
    'driver: loop {
        let next_deadline = heap.peek().map(|Reverse((t, _))| *t);

        tokio::select! {
            // `biased`: arms are polled top-down, and CANCELLATION IS FIRST.
            // With the default random order, a command that arrived before the
            // token was cancelled could win the poll against an already
            // cancelled token and `TriggerNow` would submit brand-new work into
            // a shutdown that is already under way — replying `true` in direct
            // contradiction of the handle's documented contract. The check is a
            // flag read, so the priority costs nothing.
            //
            // Priority does not starve the other arms: the deadline arm only
            // becomes ready when time passes (and its body re-arms every job it
            // drains, pushing the next deadline into the future), tick
            // completions and commands are edge-triggered.
            biased;

            // 0. Cancellation: stop arming, then wait out the ticks already in
            //    flight (below).
            _ = cancel.cancelled() => {
                break 'driver;
            }
            // 1. The earliest deadline is reached: process every due job.
            _ = wait_until(next_deadline) => {
                // Defense in depth: cancellation observed between the timer
                // firing and this body means "no new work", even though the
                // biased order above already makes this unreachable.
                if cancel.is_cancelled() {
                    break 'driver;
                }
                let now = Instant::now();
                while heap.peek().is_some_and(|Reverse((t, _))| *t <= now) {
                    let Reverse((_, idx)) = heap.pop().unwrap();
                    // The entry is consumed. Each branch below either re-arms
                    // (`arm_next`) or records that a pending tick completion
                    // will; anything that does neither leaves the job unarmed
                    // and therefore revivable by `Resume`.
                    //
                    // Clearing `next_run` HERE, at the pop that spends the
                    // deadline, is the only ordering that is truthful on every
                    // continuation: from this instant the job holds no armed
                    // deadline, so `None` is correct during tick construction
                    // (`build_tick` runs synchronously but is observable from
                    // other threads through `ScheduledJobRegistry::job()`) and
                    // for every `Submission` outcome — including `PoolClosed`,
                    // which leaves through `break 'driver` and would otherwise
                    // strand the spent instant in telemetry forever. Branches
                    // that keep the job on the clock (paused, `Concurrent`,
                    // skip-because-OOB) republish in their very next statement
                    // via `arm_next`; the resulting `None` window is a few
                    // microseconds wide and, while it lasts, accurate.
                    runtimes[idx].arm = ArmState::Unarmed;
                    registry.update_job(&runtimes[idx].name, |i| {
                        i.next_run = None;
                    });

                    // Paused: advance cadence silently, never submit.
                    if runtimes[idx].paused {
                        arm_next(idx, &mut runtimes, &mut heap, &registry, now);
                        continue;
                    }

                    match runtimes[idx].overlap {
                        // Re-arm at fire time, then submit (completion won't re-arm).
                        OverlapPolicy::Concurrent => {
                            arm_next(idx, &mut runtimes, &mut heap, &registry, now);
                            match submit_tick(idx, false, &mut runtimes, &executor, &mut in_flight, &registry) {
                                Submission::Submitted => {}
                                Submission::PoolClosed => {
                                    // The pool is closed: nothing can run
                                    // anymore. Leave through the common drain —
                                    // ticks already in flight still own the
                                    // graph.
                                    tracing::info!("Executor shut down; stopping scheduler driver");
                                    break 'driver;
                                }
                                // One job's construction is broken; the others
                                // are not. Keep driving.
                                Submission::FactoryPanicked => {
                                    disable_after_factory_panic(idx, &mut runtimes, &registry);
                                }
                            }
                        }
                        OverlapPolicy::Skip => {
                            if runtimes[idx].in_flight > 0 {
                                // An out-of-band (trigger-now) tick is still
                                // running: skip this cadence tick but keep the
                                // schedule advancing so the job fires again.
                                arm_next(idx, &mut runtimes, &mut heap, &registry, now);
                            } else {
                                match submit_tick(idx, true, &mut runtimes, &executor, &mut in_flight, &registry) {
                                    // Armed by proxy: this tick's completion
                                    // re-arms the job, so `Resume` must not
                                    // push a competing entry meanwhile. The
                                    // deadline itself is spent — the job holds
                                    // none while its tick runs, and the pop
                                    // above already cleared `next_run` to say
                                    // so. Completion republishes.
                                    Submission::Submitted => {
                                        runtimes[idx].arm = ArmState::PendingRearm;
                                    }
                                    Submission::PoolClosed => {
                                        tracing::info!("Executor shut down; stopping scheduler driver");
                                        break 'driver;
                                    }
                                    // Not re-armed: a `Skip` job re-arms on tick
                                    // completion, and there is no tick. Paused
                                    // *and* off the heap = fully disabled.
                                    Submission::FactoryPanicked => {
                                        disable_after_factory_panic(idx, &mut runtimes, &registry);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            // 2. A tick finished: update stats and (for Skip scheduled ticks) re-arm.
            Some((idx, rearm, elapsed, skipped, res)) = in_flight.next(), if !in_flight.is_empty() => {
                let panicked = res.as_ref().err().is_some_and(JoinError::is_panic);
                if panicked {
                    tracing::error!(task = %runtimes[idx].name, "Scheduled tick panicked");
                }
                runtimes[idx].in_flight = runtimes[idx].in_flight.saturating_sub(1);
                registry.update_job(&runtimes[idx].name, |i| {
                    // A predicate-skipped tick never ran its body: keep the
                    // previous body duration instead of the predicate's.
                    if !skipped {
                        i.last_duration = Some(elapsed);
                    }
                    if panicked {
                        i.panic_count += 1;
                    }
                });
                if rearm {
                    let now = Instant::now();
                    arm_next(idx, &mut runtimes, &mut heap, &registry, now);
                }
            }
            // 3. A runtime command arrived (or the channel closed → park the branch).
            cmd = next_command(&mut command_rx) => {
                // Second half of the same guarantee as `biased` above: a command
                // that somehow reached this arm after cancellation is answered
                // `false` and applied to nothing. `TriggerNow` in particular must
                // never arm new work once shutdown started; refusing here means
                // liveness does not depend on poll order alone.
                if cancel.is_cancelled() {
                    match cmd {
                        Some(Command::Pause { reply, .. })
                        | Some(Command::Resume { reply, .. })
                        | Some(Command::TriggerNow { reply, .. }) => {
                            let _ = reply.send(false);
                        }
                        None => command_rx = None,
                    }
                    continue 'driver;
                }
                match cmd {
                    Some(Command::Pause { name, reply }) => {
                        let ok = set_paused(&name, true, &mut runtimes, &registry);
                        let _ = reply.send(ok);
                    }
                    Some(Command::Resume { name, reply }) => {
                        // THE CONTRACT: a paused job never fires; `resume`
                        // replies `true` iff the job can fire again, as far as
                        // the driver can tell when the command is handled.
                        // Three states, three answers:
                        //
                        // - `Deadline` — a live heap entry (the ordinary
                        //   pause → resume, and a `Concurrent`/trigger-now
                        //   factory panic, which disables the job *after* the
                        //   cadence entry was pushed). Kept as is: `true`.
                        // - `PendingRearm` — a `Skip` tick in flight will
                        //   re-arm on completion, but only if the schedule
                        //   still yields a fire then. Pushing a deadline now
                        //   would double-arm (and could overlap the running
                        //   tick), so instead the schedule is PROBED on a clone
                        //   of the re-arm state: `Some` → `true`, exhausted →
                        //   `false`. The probe is a snapshot, not a promise: a
                        //   cron's last slot can pass between here and the
                        //   completion, and no implementation can rule that
                        //   out. The end state stays honest either way — the
                        //   completion's `arm_next` leaves the job `Unarmed`
                        //   with `next_run: None`.
                        // - `Unarmed` — re-arm from now (interval: now+period,
                        //   cron: next matching slot) and report the outcome. A
                        //   schedule that can never fire again stays unarmed:
                        //   the paused flag is cleared, the reply is `false`.
                        let ok = match runtimes.iter().position(|j| j.name == name) {
                            None => false,
                            Some(idx) => {
                                set_paused(&name, false, &mut runtimes, &registry);
                                match runtimes[idx].arm {
                                    ArmState::Deadline => true,
                                    ArmState::PendingRearm => {
                                        let mut probe = runtimes[idx].rearm.clone();
                                        compute_next(&mut probe, Instant::now(), &name).is_some()
                                    }
                                    ArmState::Unarmed => {
                                        rearm_from_now(idx, &mut runtimes, &mut heap, &registry);
                                        runtimes[idx].arm == ArmState::Deadline
                                    }
                                }
                            }
                        };
                        let _ = reply.send(ok);
                    }
                    Some(Command::TriggerNow { name, reply }) => {
                        // Set when the submission found the pool closed: the
                        // caller is answered FIRST (a dropped reply sender
                        // would read as `false` anyway, but the contract is an
                        // explicit answer), then the driver leaves through the
                        // common drain.
                        let mut pool_closed = false;
                        let ok = match runtimes.iter().position(|j| j.name == name) {
                            None => false,
                            // A Skip job already running refuses the extra tick.
                            Some(idx)
                                if matches!(runtimes[idx].overlap, OverlapPolicy::Skip)
                                    && runtimes[idx].in_flight > 0 =>
                            {
                                false
                            }
                            // OOB tick never re-arms; the regular heap entry is untouched.
                            Some(idx) => match submit_tick(
                                idx, false, &mut runtimes, &executor, &mut in_flight, &registry,
                            ) {
                                Submission::Submitted => true,
                                // Terminal, like every other submission site: a
                                // closed pool means nothing can ever run again.
                                // Waiting for a cadence fire to discover it is
                                // not enough — every job may be paused or
                                // exhausted, and the driver would park forever
                                // holding the bean graph alive.
                                Submission::PoolClosed => {
                                    pool_closed = true;
                                    false
                                }
                                Submission::FactoryPanicked => {
                                    disable_after_factory_panic(idx, &mut runtimes, &registry);
                                    false
                                }
                            },
                        };
                        let _ = reply.send(ok);
                        if pool_closed {
                            tracing::info!("Executor shut down; stopping scheduler driver");
                            break 'driver;
                        }
                    }
                    // Sender dropped: disable the branch so it can't busy-loop.
                    None => command_rx = None,
                }
            }
        }
    }

    // Close the command channel BEFORE draining. During the drain the driver no
    // longer polls commands, but `SchedulerHandle::{pause,resume,trigger_now}`
    // enqueue and then await a oneshot REPLY: a tick body that calls one while
    // it is itself being drained would wait for a driver that is waiting for
    // it — a deadlock bounded only by `shutdown_grace_period`, after which
    // `run()` returns with a graph-owning task still alive. Dropping the
    // receiver makes both halves fail fast instead: an enqueued command's reply
    // sender is dropped (the awaited oneshot resolves to `Err` → `false`) and a
    // new send hits a closed channel (→ `false`). Commands are no-ops from here
    // on by design — the schedule is over.
    drop(command_rx.take());

    // Ticks already submitted are NOT aborted — and the driver does not return
    // until they are done. That matters beyond tidiness: the driver runs as a
    // tracked task, so while it is alive the bean graph is alive; a tick body
    // resolving a `GraphHandle` (a tenant-cascade `#[scheduled]` method, say)
    // would otherwise be the one piece of scheduler work still running with
    // nobody keeping the graph up — the executor's own pool drain only covers
    // it on a clean shutdown, not on an aborted boot.
    //
    // Per-job stats are deliberately not updated here: the schedule is over,
    // and `run_count`/`last_run` for a tick that outlived cancellation would be
    // read by nobody. On a clean shutdown this loop is already empty — the
    // executor drained the pool before the framework joined this task.
    if !in_flight.is_empty() {
        tracing::debug!(count = in_flight.len(), "Draining in-flight scheduled ticks");
        while in_flight.next().await.is_some() {}
    }

    // The driver is the only thing that can fire a deadline, and it is about to
    // return. Every entry left on the heap — cancellation cut the loop, or a
    // closed pool did — is a fire that will not happen, so publishing it would
    // be the same staleness as an unpopped spent instant, only wider. Decision:
    // clear ALL of them on the way out, which makes the rule global and
    // driver-scoped: `next_run` is `Some` only while a live driver holds that
    // deadline; once the scheduler has stopped, every job reads `None`.
    for job in &runtimes {
        registry.update_job(&job.name, |i| {
            i.next_run = None;
        });
    }

    tracing::info!(count, "Scheduler driver stopped");
}
