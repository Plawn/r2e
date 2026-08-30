//! `WorkerSet` — the aggregated lifecycle and metrics of every sharded worker.
//!
//! Provide one as a bean to observe the workers from anywhere:
//!
//! ```ignore
//! let workers = WorkerSet::new();
//! AppBuilder::new()
//!     .provide(workers.clone())         // picked up by run(): states flow in
//!     .plugin(AdvancedHealth::new())    // provides HealthRegistry
//!     .plugin(WorkerHealth::new())      // readiness = every worker Serving
//! ```
//!
//! Each worker owns a [`WorkerSlot`]: its [`WorkerState`], effective CPU,
//! local/remote crossing counters (fed by [`Mailboxes`](super::mailbox)),
//! mailbox occupancy/wait, and the error that took it to
//! [`WorkerState::Failed`]. Reads are lock-free ([`WorkerSet::snapshot`]);
//! writers are the worker bootstrap in sharded serving and the test harness.
//!
//! A `WorkerSet` that is never handed to `run()` (or configured by the
//! harness) has zero slots: [`WorkerSet::all_serving`] is `true` and the
//! readiness indicator reports `Up`, because a non-sharded app has no worker
//! that could be down.

use std::sync::atomic::{AtomicI64, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arc_swap::ArcSwap;

use crate::builtins::health::{HealthIndicator, HealthRegistry, HealthStatus};
use crate::plugin::{Plugin, PluginBuildContext, PluginBuildError};
use crate::rt::sync::Notify;

/// Lifecycle state of one sharded worker.
///
/// Transitions, in order, on the happy path:
/// `Unstarted → Starting → Ready → Serving → Draining → ServicesDown → Parked → Exited`.
/// Any state may jump to `Failed` (startup error, serve error, panic).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum WorkerState {
    /// Slot allocated, thread not spawned yet.
    Unstarted = 0,
    /// Thread and runtime up; per-worker services starting.
    Starting = 1,
    /// Services started; waiting at the all-or-nothing startup barrier.
    Ready = 2,
    /// Accepting connections.
    Serving = 3,
    /// Shutdown signalled; HTTP connections draining, services still up.
    Draining = 4,
    /// HTTP drained and every per-worker service shut down.
    ServicesDown = 5,
    /// Parked: runtime kept alive for upgraded sockets until released.
    Parked = 6,
    /// Thread returned cleanly.
    Exited = 7,
    /// Thread returned with an error (see [`WorkerSnapshot::error`]).
    Failed = 8,
}

impl WorkerState {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Unstarted,
            1 => Self::Starting,
            2 => Self::Ready,
            3 => Self::Serving,
            4 => Self::Draining,
            5 => Self::ServicesDown,
            6 => Self::Parked,
            7 => Self::Exited,
            _ => Self::Failed,
        }
    }

    /// Stable lowercase label for metrics/logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unstarted => "unstarted",
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Serving => "serving",
            Self::Draining => "draining",
            Self::ServicesDown => "services_down",
            Self::Parked => "parked",
            Self::Exited => "exited",
            Self::Failed => "failed",
        }
    }

    /// Every state, in transition order — for metric label enumeration.
    pub const ALL: [WorkerState; 9] = [
        Self::Unstarted,
        Self::Starting,
        Self::Ready,
        Self::Serving,
        Self::Draining,
        Self::ServicesDown,
        Self::Parked,
        Self::Exited,
        Self::Failed,
    ];
}

impl std::fmt::Display for WorkerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Live counters of one worker. Obtained through [`WorkerSet::slot`].
pub struct WorkerSlot {
    id: usize,
    state: AtomicU8,
    cpu: AtomicI64,
    local_crossings: AtomicU64,
    remote_crossings: AtomicU64,
    mailbox_depth: AtomicUsize,
    mailbox_sends: AtomicU64,
    mailbox_wait_nanos: AtomicU64,
    error: Mutex<Option<String>>,
    notify: Arc<Notify>,
}

impl std::fmt::Debug for WorkerSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.snapshot().fmt(f)
    }
}

impl WorkerSlot {
    fn new(id: usize, notify: Arc<Notify>) -> Self {
        Self {
            id,
            state: AtomicU8::new(WorkerState::Unstarted as u8),
            cpu: AtomicI64::new(-1),
            local_crossings: AtomicU64::new(0),
            remote_crossings: AtomicU64::new(0),
            mailbox_depth: AtomicUsize::new(0),
            mailbox_sends: AtomicU64::new(0),
            mailbox_wait_nanos: AtomicU64::new(0),
            error: Mutex::new(None),
            notify,
        }
    }

    /// Worker index.
    pub fn id(&self) -> usize {
        self.id
    }

    /// Current lifecycle state.
    pub fn state(&self) -> WorkerState {
        WorkerState::from_u8(self.state.load(Ordering::Acquire))
    }

    /// Record a transition. Framework/harness use.
    #[doc(hidden)]
    pub fn set_state(&self, state: WorkerState) {
        let prev = self.state.swap(state as u8, Ordering::AcqRel);
        if prev != state as u8 {
            tracing::debug!(worker = self.id, from = %WorkerState::from_u8(prev), to = %state, "worker state");
            self.notify.notify_waiters();
        }
    }

    /// Record a failure: sets [`WorkerState::Failed`] and keeps `error`.
    #[doc(hidden)]
    pub fn fail(&self, error: impl Into<String>) {
        *self.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(error.into());
        self.set_state(WorkerState::Failed);
    }

    /// Effective CPU affinity (`None` until pinning lands).
    pub fn cpu(&self) -> Option<usize> {
        usize::try_from(self.cpu.load(Ordering::Relaxed)).ok()
    }

    #[doc(hidden)]
    pub fn set_cpu(&self, cpu: Option<usize>) {
        self.cpu
            .store(cpu.map_or(-1, |c| c as i64), Ordering::Relaxed);
    }

    /// Count a message delivered to this worker's mailbox from its own
    /// thread (`local == true`) or from anywhere else.
    pub fn record_crossing(&self, local: bool) {
        if local {
            self.local_crossings.fetch_add(1, Ordering::Relaxed);
        } else {
            self.remote_crossings.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Mailbox bookkeeping: one message queued.
    pub fn mailbox_enqueued(&self) {
        self.mailbox_depth.fetch_add(1, Ordering::Relaxed);
        self.mailbox_sends.fetch_add(1, Ordering::Relaxed);
    }

    /// Mailbox bookkeeping: one message dequeued after waiting `waited`.
    pub fn mailbox_dequeued(&self, waited: Duration) {
        self.mailbox_depth.fetch_sub(1, Ordering::Relaxed);
        self.mailbox_wait_nanos
            .fetch_add(waited.as_nanos() as u64, Ordering::Relaxed);
    }

    /// Point-in-time copy of every counter.
    pub fn snapshot(&self) -> WorkerSnapshot {
        WorkerSnapshot {
            id: self.id,
            state: self.state(),
            cpu: self.cpu(),
            local_crossings: self.local_crossings.load(Ordering::Relaxed),
            remote_crossings: self.remote_crossings.load(Ordering::Relaxed),
            mailbox_depth: self.mailbox_depth.load(Ordering::Relaxed),
            mailbox_sends: self.mailbox_sends.load(Ordering::Relaxed),
            mailbox_wait_total: Duration::from_nanos(
                self.mailbox_wait_nanos.load(Ordering::Relaxed),
            ),
            error: self.error.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        }
    }
}

fn duration_secs<S: serde::Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_f64(d.as_secs_f64())
}

/// Point-in-time copy of a [`WorkerSlot`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WorkerSnapshot {
    /// Worker index.
    pub id: usize,
    /// Lifecycle state.
    pub state: WorkerState,
    /// Effective CPU affinity.
    pub cpu: Option<usize>,
    /// Mailbox messages sent to this worker from its own thread.
    pub local_crossings: u64,
    /// Mailbox messages sent to this worker from another thread (the control
    /// plane or another worker) — the "not shared-nothing" signal.
    pub remote_crossings: u64,
    /// Messages currently queued in this worker's mailbox.
    pub mailbox_depth: usize,
    /// Messages ever queued in this worker's mailbox.
    pub mailbox_sends: u64,
    /// Total time messages spent queued before being received.
    /// Serialised as fractional seconds (`mailbox_wait_seconds`).
    #[serde(rename = "mailbox_wait_seconds", serialize_with = "duration_secs")]
    pub mailbox_wait_total: Duration,
    /// The error that moved the worker to [`WorkerState::Failed`].
    pub error: Option<String>,
}

struct Inner {
    slots: ArcSwap<Vec<Arc<WorkerSlot>>>,
    notify: Arc<Notify>,
}

/// Aggregated view of every sharded worker. Cheap to clone (an `Arc`).
#[derive(Clone)]
pub struct WorkerSet {
    inner: Arc<Inner>,
}

impl Default for WorkerSet {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for WorkerSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.snapshot()).finish()
    }
}

impl WorkerSet {
    /// An empty set. `run()` sizes it to `server.workers` when it is provided
    /// as a bean; the harness sizes it itself.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                slots: ArcSwap::from_pointee(Vec::new()),
                notify: Arc::new(Notify::new()),
            }),
        }
    }

    /// Size the set to `workers` slots, all [`WorkerState::Unstarted`].
    /// A no-op when already sized to `workers` (so `prepare()` and
    /// `serve_sharded` can both call it); resizing resets every counter.
    #[doc(hidden)]
    pub fn configure(&self, workers: usize) {
        if self.inner.slots.load().len() == workers {
            return;
        }
        let slots = (0..workers)
            .map(|i| Arc::new(WorkerSlot::new(i, Arc::clone(&self.inner.notify))))
            .collect();
        self.inner.slots.store(Arc::new(slots));
        self.inner.notify.notify_waiters();
    }

    /// Number of slots (`0` for a set no sharded server has claimed).
    pub fn workers(&self) -> usize {
        self.inner.slots.load().len()
    }

    /// The slot of worker `id`, if it exists. Lock-free.
    pub fn slot(&self, id: usize) -> Option<Arc<WorkerSlot>> {
        self.inner.slots.load().get(id).cloned()
    }

    /// Point-in-time copy of every worker.
    pub fn snapshot(&self) -> Vec<WorkerSnapshot> {
        self.inner
            .slots
            .load()
            .iter()
            .map(|s| s.snapshot())
            .collect()
    }

    /// Current state of every worker, by index.
    pub fn states(&self) -> Vec<WorkerState> {
        self.inner.slots.load().iter().map(|s| s.state()).collect()
    }

    /// `true` when every worker is in `state` (vacuously `true` with no
    /// slots).
    pub fn all_in(&self, state: WorkerState) -> bool {
        self.inner.slots.load().iter().all(|s| s.state() == state)
    }

    /// `true` when every worker is [`WorkerState::Serving`].
    pub fn all_serving(&self) -> bool {
        self.all_in(WorkerState::Serving)
    }

    /// `true` when any worker is [`WorkerState::Failed`].
    pub fn any_failed(&self) -> bool {
        self.inner
            .slots
            .load()
            .iter()
            .any(|s| s.state() == WorkerState::Failed)
    }

    /// The first failure recorded, if any: `(worker, error)`.
    pub fn first_error(&self) -> Option<(usize, String)> {
        self.inner
            .slots
            .load()
            .iter()
            .find_map(|s| s.snapshot().error.map(|e| (s.id, e)))
    }

    /// Resolve once `pred(self)` holds; re-checked on every state transition.
    pub async fn wait_until(&self, pred: impl Fn(&WorkerSet) -> bool) {
        loop {
            let notified = self.inner.notify.notified();
            crate::rt::pin!(notified);
            // Arm before checking so a transition between the check and the
            // await is not lost.
            notified.as_mut().enable();
            if pred(self) {
                return;
            }
            notified.await;
        }
    }

    /// Resolve once every worker is [`WorkerState::Serving`].
    pub async fn wait_all_serving(&self) {
        self.wait_until(|s| s.all_serving()).await
    }

    /// Resolve once every worker has left the set for good
    /// ([`WorkerState::Exited`] or [`WorkerState::Failed`]).
    pub async fn wait_all_exited(&self) {
        self.wait_until(|s| {
            s.inner
                .slots
                .load()
                .iter()
                .all(|w| matches!(w.state(), WorkerState::Exited | WorkerState::Failed))
        })
        .await
    }
}

// ── Readiness plugin ─────────────────────────────────────────────────────────

/// Readiness indicator over a [`WorkerSet`]: `Up` only while **every** worker
/// is [`WorkerState::Serving`] (vacuously `Up` for a non-sharded app).
///
/// `Deps = (WorkerSet, HealthRegistry)`: provide the set (the same clone
/// `run()` drives) and install [`AdvancedHealth`](crate::builtins::AdvancedHealth)
/// first. The first worker to enter `Draining` flips `/health/ready` to 503,
/// so a load balancer deregisters the instance at the start of shutdown, not
/// at the end.
#[derive(Debug, Clone)]
pub struct WorkerHealth {
    name: String,
}

impl Default for WorkerHealth {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkerHealth {
    /// Indicator named `"workers"`.
    pub fn new() -> Self {
        Self {
            name: "workers".to_string(),
        }
    }

    /// Override the indicator name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
}

impl Plugin for WorkerHealth {
    type Provided = ();
    type Deps = (WorkerSet, HealthRegistry);
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        (set, registry): Self::Deps,
        _config: Option<Self::Config>,
        ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        if !ctx.enabled() {
            return Ok(());
        }
        registry.register(WorkerReadiness {
            name: self.name,
            set,
        });
        Ok(())
    }
}

struct WorkerReadiness {
    name: String,
    set: WorkerSet,
}

impl HealthIndicator for WorkerReadiness {
    fn name(&self) -> &str {
        &self.name
    }

    async fn check(&self) -> HealthStatus {
        let not_serving: Vec<String> = self
            .set
            .snapshot()
            .into_iter()
            .filter(|w| w.state != WorkerState::Serving)
            .map(|w| match w.error {
                Some(e) => format!("worker {}: {} ({e})", w.id, w.state),
                None => format!("worker {}: {}", w.id, w.state),
            })
            .collect();
        if not_serving.is_empty() {
            HealthStatus::Up
        } else {
            HealthStatus::Down(not_serving.join(", "))
        }
    }
}

impl serde::Serialize for WorkerState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
