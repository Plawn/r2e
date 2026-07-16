//! The single-driver scheduler runtime.
//!
//! Instead of spawning one Tokio task per schedule, the scheduler spawns
//! exactly ONE driver task ([`start_jobs`]) that owns every schedule. The
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
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use r2e_core::rt::JoinError;
use r2e_executor::PoolExecutor;

use crate::types::{OverlapPolicy, ScheduleConfig, ScheduledJob};
use crate::ScheduledJobRegistry;

/// A runtime control command delivered to the driver via [`SchedulerHandle`].
///
/// Each command carries a oneshot reply channel; the boolean answer reports
/// whether the command applied (`false` = unknown job, or a `Skip` job that was
/// already running for `TriggerNow`).
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

/// Start the scheduler driver for `jobs`.
///
/// Spawns exactly ONE task (via [`r2e_core::rt::spawn`]) that owns all
/// schedules: a min-heap of next-fire deadlines drives when each job's tick
/// body is submitted to the shared `executor`. This is the single entry point
/// used both by the [`Scheduler`](crate::Scheduler) plugin and by tests.
///
/// `registry` receives live stats updates (pass a fresh
/// [`ScheduledJobRegistry`] if you don't care). `commands` carries runtime
/// control; use [`SchedulerCommands::disconnected`] when none is wired.
///
/// Ticks run on the pool (not inline), so a panicking tick is contained in its
/// pool job and the driver keeps ticking. When the pool rejects a submission
/// (it has shut down), the driver stops — nothing can run anymore. On
/// cancellation the driver breaks without aborting in-flight ticks; the
/// executor's graceful shutdown drains them.
pub fn start_jobs(
    jobs: Vec<ScheduledJob>,
    cancel: CancellationToken,
    executor: PoolExecutor,
    registry: ScheduledJobRegistry,
    commands: SchedulerCommands,
) {
    r2e_core::rt::spawn(async move {
        run_driver(jobs, cancel, executor, registry, commands).await;
    });
}

/// How a job computes its next fire time when it is re-armed.
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
    rearm: Rearm,
    overlap: OverlapPolicy,
    /// Paused jobs advance their cadence but never submit a scheduled tick.
    paused: bool,
    /// Number of ticks of this job currently running on the pool.
    in_flight: usize,
}

/// In-flight tick result: `(job index, re-arm on completion, wall duration, join result)`.
type InFlight =
    FuturesUnordered<Pin<Box<dyn Future<Output = (usize, bool, Duration, Result<(), JoinError>)> + Send>>>;

/// The next upcoming cron fire time as a tokio [`Instant`], or `None` if the
/// schedule has no further executions.
fn cron_next_instant(schedule: &cron::Schedule) -> Option<Instant> {
    let now_utc = Utc::now();
    let next = schedule.upcoming(Utc).next()?;
    let until = (next - now_utc).to_std().unwrap_or(Duration::ZERO);
    Some(Instant::now() + until)
}

/// Project a tokio [`Instant`] onto wall-clock time for user-facing stats.
fn instant_to_datetime(t: Instant) -> DateTime<Utc> {
    let now_inst = Instant::now();
    let now_utc = Utc::now();
    if t >= now_inst {
        now_utc + chrono::Duration::from_std(t - now_inst).unwrap_or_default()
    } else {
        now_utc - chrono::Duration::from_std(now_inst - t).unwrap_or_default()
    }
}

/// Advance a job's `rearm` state and return its next fire instant (`None` for an
/// exhausted cron schedule).
fn compute_next(rearm: &mut Rearm, now: Instant, name: &str) -> Option<Instant> {
    match rearm {
        Rearm::Interval { period, deadline } => {
            let mut next = *deadline + *period;
            while next <= now {
                next += *period;
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

/// Submit one tick of job `idx` to the pool. `rearm` records whether the tick's
/// completion should re-arm the job (true only for `Skip` scheduled ticks).
/// Returns `false` when the pool has shut down (nothing can run anymore).
fn submit_tick(
    idx: usize,
    rearm: bool,
    runtimes: &mut [JobRuntime],
    executor: &PoolExecutor,
    in_flight: &mut InFlight,
    registry: &ScheduledJobRegistry,
) -> bool {
    let fut = (runtimes[idx].run)();
    match executor.submit(fut) {
        Ok(handle) => {
            runtimes[idx].in_flight += 1;
            let start = Instant::now();
            registry.update_job(&runtimes[idx].name, |i| {
                i.last_run = Some(instant_to_datetime(start));
                i.run_count += 1;
            });
            in_flight.push(Box::pin(async move {
                let res = handle.await;
                (idx, rearm, start.elapsed(), res)
            }));
            true
        }
        Err(_) => false,
    }
}

/// Push a job's next deadline onto the heap and mirror it into the registry.
fn arm_next(idx: usize, runtimes: &mut [JobRuntime], heap: &mut BinaryHeap<Reverse<(Instant, usize)>>, registry: &ScheduledJobRegistry, now: Instant) {
    let next = compute_next(&mut runtimes[idx].rearm, now, &runtimes[idx].name);
    if let Some(t) = next {
        heap.push(Reverse((t, idx)));
    }
    registry.update_job(&runtimes[idx].name, |i| {
        i.next_run = next.map(instant_to_datetime);
    });
}

/// Set/clear a job's paused flag. Returns `false` for an unknown job.
fn set_paused(name: &str, paused: bool, runtimes: &mut [JobRuntime], registry: &ScheduledJobRegistry) -> bool {
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
                    period: *period,
                    deadline: now,
                },
                Some(now),
            ),
            ScheduleConfig::IntervalWithDelay {
                interval,
                initial_delay,
            } => {
                let first = now + *initial_delay;
                (
                    Rearm::Interval {
                        period: *interval,
                        deadline: first,
                    },
                    Some(first),
                )
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
            rearm,
            overlap: job.overlap,
            paused: false,
            in_flight: 0,
        });
    }

    let count = runtimes.len();
    tracing::info!(count, "Scheduler driver started");

    let mut in_flight: InFlight = FuturesUnordered::new();

    loop {
        let next_deadline = heap.peek().map(|Reverse((t, _))| *t);

        tokio::select! {
            // 1. The earliest deadline is reached: process every due job.
            _ = wait_until(next_deadline) => {
                let now = Instant::now();
                while heap.peek().is_some_and(|Reverse((t, _))| *t <= now) {
                    let Reverse((_, idx)) = heap.pop().unwrap();

                    // Paused: advance cadence silently, never submit.
                    if runtimes[idx].paused {
                        arm_next(idx, &mut runtimes, &mut heap, &registry, now);
                        continue;
                    }

                    match runtimes[idx].overlap {
                        // Re-arm at fire time, then submit (completion won't re-arm).
                        OverlapPolicy::Concurrent => {
                            arm_next(idx, &mut runtimes, &mut heap, &registry, now);
                            if !submit_tick(idx, false, &mut runtimes, &executor, &mut in_flight, &registry) {
                                tracing::info!("Executor shut down; stopping scheduler driver");
                                tracing::info!(count, "Scheduler driver stopped");
                                return;
                            }
                        }
                        OverlapPolicy::Skip => {
                            if runtimes[idx].in_flight > 0 {
                                // An out-of-band (trigger-now) tick is still
                                // running: skip this cadence tick but keep the
                                // schedule advancing so the job fires again.
                                arm_next(idx, &mut runtimes, &mut heap, &registry, now);
                            } else if !submit_tick(idx, true, &mut runtimes, &executor, &mut in_flight, &registry) {
                                tracing::info!("Executor shut down; stopping scheduler driver");
                                tracing::info!(count, "Scheduler driver stopped");
                                return;
                            }
                        }
                    }
                }
            }
            // 2. A tick finished: update stats and (for Skip scheduled ticks) re-arm.
            Some((idx, rearm, elapsed, res)) = in_flight.next(), if !in_flight.is_empty() => {
                let panicked = res.as_ref().err().is_some_and(JoinError::is_panic);
                if panicked {
                    tracing::error!(task = %runtimes[idx].name, "Scheduled tick panicked");
                }
                runtimes[idx].in_flight = runtimes[idx].in_flight.saturating_sub(1);
                registry.update_job(&runtimes[idx].name, |i| {
                    i.last_duration = Some(elapsed);
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
                match cmd {
                    Some(Command::Pause { name, reply }) => {
                        let ok = set_paused(&name, true, &mut runtimes, &registry);
                        let _ = reply.send(ok);
                    }
                    Some(Command::Resume { name, reply }) => {
                        let ok = set_paused(&name, false, &mut runtimes, &registry);
                        let _ = reply.send(ok);
                    }
                    Some(Command::TriggerNow { name, reply }) => {
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
                            Some(idx) => submit_tick(
                                idx, false, &mut runtimes, &executor, &mut in_flight, &registry,
                            ),
                        };
                        let _ = reply.send(ok);
                    }
                    // Sender dropped: disable the branch so it can't busy-loop.
                    None => command_rx = None,
                }
            }
            // 4. Cancellation: stop without aborting in-flight ticks.
            _ = cancel.cancelled() => {
                break;
            }
        }
    }

    tracing::info!(count, "Scheduler driver stopped");
}
