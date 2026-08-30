//! Per-worker services — shard-local, `!Send` services that live inside a
//! sharded HTTP worker's `current_thread` runtime.
//!
//! Sharded serving (`server.workers`, see [`super::sharded`]) runs HTTP on `N`
//! worker threads, each with its own `current_thread` runtime. Everything
//! *else* R2E manages (scheduler, executor, consumers, QUIC) runs on the
//! control plane. Applications that need strict thread-per-core ownership of a
//! **non-HTTP** workload — one QUIC endpoint per shard with its connection
//! state, a per-shard UDP socket, a shard-local registry or mailbox — register
//! a **per-worker service factory** on the builder:
//!
//! ```ignore
//! AppBuilder::new()
//!     .per_worker_service(|worker: WorkerContext| async move {
//!         // Runs exactly once inside worker `worker.id()`'s runtime, after the
//!         // runtime exists and BEFORE the worker accepts its first connection.
//!         // `Rc<RefCell<_>>` and any other `!Send` state is valid here and in
//!         // every task spawned through `worker.spawn_local(..)`.
//!         let state = std::rc::Rc::new(std::cell::RefCell::new(0u64));
//!         let shutdown = worker.shutdown();
//!         worker.spawn_local(async move {
//!             shutdown.cancelled().await;
//!         });
//!         Ok::<_, BoxError>(MyShardService { state })
//!     })
//! ```
//!
//! # Guarantees
//!
//! - The factory runs **exactly once per worker**, on the worker's own OS
//!   thread, inside its `current_thread` runtime and inside a
//!   [`LocalSet`](crate::rt::LocalSet) — after the control-plane handle is
//!   registered and before the worker starts serving HTTP. Factories run in
//!   registration order.
//! - The factory future, the returned service, and everything spawned via
//!   [`WorkerContext::spawn_local`] stay on that thread for their whole life.
//!   [`WorkerContext`] itself is `!Send + !Sync`, so nothing built from it can
//!   be moved to the control plane or to another worker by accident.
//! - Startup is **all-or-nothing**: no worker accepts traffic until every
//!   worker has started every service. If any factory fails (or panics), the
//!   failing worker shuts down the services it already started (reverse order),
//!   every other worker is cancelled and does the same, and `run()` returns an
//!   error naming the worker and the service index.
//! - Graceful shutdown per worker: the worker's shutdown token
//!   ([`WorkerContext::shutdown`]) is cancelled, in-flight HTTP connections
//!   drain, then each service's [`WorkerService::shutdown`] is awaited in
//!   reverse start order, then the `LocalSet` and the runtime are dropped
//!   (cancelling any still-running local task). The service is never dropped
//!   while its `shutdown` future is pending.
//! - The factory itself is created on the main thread and shared by all
//!   workers, so it must be `Send + Sync` (typically it captures `Arc`s and
//!   config); everything it *produces* may be `!Send`.
//!
//! Per-worker services require sharded serving: registering one without
//! `server.workers` is a hard error at `run()` — there is no worker runtime to
//! host it on the single-listener path, and running it on the multi-thread
//! control plane would silently break the `!Send` ownership promise.
//!
//! CPU affinity: R2E does not pin worker threads today, so
//! [`WorkerContext::cpu`] is `None`. The slot exists so consumers written
//! against it keep working when pinning lands.

use std::cell::Cell;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::ThreadId;

use crate::rt::{CancelToken, JobHandle};

/// Boxed error type accepted from per-worker service factories.
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// A boxed `!Send` future, pinned to the worker thread.
pub type LocalBoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// A service owned by one sharded worker.
///
/// Implement this for the value your factory returns. The only method is the
/// cleanup hook; a service with nothing to clean up can return `()` from the
/// factory instead (`()` implements this trait with a no-op shutdown).
///
/// The service is `!Send`-capable: it may own `Rc`, `RefCell`, sockets adopted
/// into the worker runtime, and handles of tasks spawned with
/// [`WorkerContext::spawn_local`].
pub trait WorkerService: 'static {
    /// Graceful cleanup, awaited by the worker after HTTP drain completes and
    /// before the worker runtime is torn down. Services shut down in reverse
    /// start order. Runs on the worker thread; may await local tasks.
    fn shutdown(self: Box<Self>) -> LocalBoxFuture<'static, ()> {
        Box::pin(async {})
    }
}

impl WorkerService for () {}

/// Type-erased, shareable per-worker service factory.
///
/// Built by [`crate::builder::AppBuilder::per_worker_service`]; one clone is
/// carried into every worker. Invoked exactly once per worker with that
/// worker's [`WorkerContext`].
#[derive(Clone)]
pub struct PerWorkerServiceFactory {
    inner: Arc<dyn FactoryFn>,
}

/// Type-erased factory closure: `WorkerContext` → boxed `WorkerService`.
trait FactoryFn:
    Fn(WorkerContext) -> LocalBoxFuture<'static, Result<Box<dyn WorkerService>, BoxError>> + Send + Sync
{
}

impl<F> FactoryFn for F where
    F: Fn(WorkerContext) -> LocalBoxFuture<'static, Result<Box<dyn WorkerService>, BoxError>>
        + Send
        + Sync
{
}

impl std::fmt::Debug for PerWorkerServiceFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PerWorkerServiceFactory")
    }
}

impl PerWorkerServiceFactory {
    /// Wrap a user factory.
    ///
    /// `f` runs on the worker thread; the future it returns and the service it
    /// resolves to need not be `Send`.
    pub fn new<F, Fut, S>(f: F) -> Self
    where
        F: Fn(WorkerContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<S, BoxError>> + 'static,
        S: WorkerService,
    {
        Self {
            inner: Arc::new(move |ctx| {
                let fut = f(ctx);
                Box::pin(async move { fut.await.map(|s| Box::new(s) as Box<dyn WorkerService>) })
            }),
        }
    }

    /// Run the factory for `ctx`'s worker. Must be called on that worker's
    /// thread, inside its `LocalSet`.
    pub fn build(
        &self,
        ctx: WorkerContext,
    ) -> LocalBoxFuture<'static, Result<Box<dyn WorkerService>, BoxError>> {
        (self.inner)(ctx)
    }
}

/// Identity and local facilities of the sharded worker a service runs on.
///
/// Handed to each [`PerWorkerServiceFactory`]; clone it into the service and
/// into local tasks. `!Send + !Sync` by construction: a `WorkerContext` can
/// only ever be used on the worker thread that created it, which is what makes
/// [`spawn_local`](Self::spawn_local) safe to expose.
#[derive(Clone)]
pub struct WorkerContext {
    id: usize,
    workers: usize,
    cpu: Option<usize>,
    thread: ThreadId,
    shutdown: CancelToken,
    /// Pins the type to the worker thread (`Cell` is `!Sync`, the raw pointer
    /// is `!Send`).
    _local: PhantomData<Cell<*const ()>>,
}

impl std::fmt::Debug for WorkerContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerContext")
            .field("id", &self.id)
            .field("workers", &self.workers)
            .field("cpu", &self.cpu)
            .field("thread", &self.thread)
            .finish_non_exhaustive()
    }
}

impl WorkerContext {
    /// Construct a context for worker `id`. Called by the worker bootstrap on
    /// the worker thread itself (the `ThreadId` is captured from the caller).
    #[doc(hidden)]
    pub fn new(id: usize, workers: usize, cpu: Option<usize>, shutdown: CancelToken) -> Self {
        Self {
            id,
            workers,
            cpu,
            thread: std::thread::current().id(),
            shutdown,
            _local: PhantomData,
        }
    }

    /// Stable, zero-based worker (shard) index in `0..workers()`. Also the
    /// suffix of the worker's thread name (`r2e-worker-{id}`).
    pub fn id(&self) -> usize {
        self.id
    }

    /// Total number of workers in this server.
    pub fn workers(&self) -> usize {
        self.workers
    }

    /// The CPU this worker is pinned to, when known. Currently always `None`:
    /// R2E does not apply CPU affinity to worker threads.
    pub fn cpu(&self) -> Option<usize> {
        self.cpu
    }

    /// The OS thread this worker runs on.
    pub fn thread_id(&self) -> ThreadId {
        self.thread
    }

    /// This worker's plane. A `WorkerContext` always belongs to a data-plane
    /// worker (the control plane has no context — it is the caller's
    /// multi-thread runtime).
    pub fn role(&self) -> WorkerRole {
        WorkerRole::DataPlane
    }

    /// The sendable identity of this worker: the same `id`/`workers`/`cpu`
    /// as a `Copy + Send + Sync` value, for labelling work that leaves the
    /// worker thread (metrics, mailbox envelopes, log fields).
    pub fn info(&self) -> WorkerInfo {
        WorkerInfo {
            id: self.id,
            workers: self.workers,
            cpu: self.cpu,
            role: WorkerRole::DataPlane,
        }
    }

    /// The worker's shutdown token: cancelled when graceful shutdown begins
    /// (at the same moment the worker's HTTP listener stops accepting), and
    /// also when startup is aborted because another worker failed. Local
    /// tasks should `select!` on `shutdown().cancelled()` to exit promptly;
    /// [`WorkerService::shutdown`] then runs once HTTP drain is complete.
    pub fn shutdown(&self) -> CancelToken {
        self.shutdown.clone()
    }

    /// Spawn a `!Send` task on this worker's local executor.
    ///
    /// The task runs on this worker thread only — it may hold `Rc`/`RefCell`
    /// captured from the factory or the service. Tasks still running when the
    /// worker's services have shut down are cancelled (dropped) with the
    /// worker's `LocalSet`; await them from [`WorkerService::shutdown`] if they
    /// must finish.
    ///
    /// # Panics
    ///
    /// If called from a thread other than the worker's own (impossible for a
    /// `WorkerContext` obtained from the factory, since the type is `!Send`),
    /// or outside the worker's `LocalSet` context.
    pub fn spawn_local<F>(&self, future: F) -> JobHandle<F::Output>
    where
        F: Future + 'static,
        F::Output: 'static,
    {
        assert_eq!(
            std::thread::current().id(),
            self.thread,
            "WorkerContext::spawn_local called off worker {} thread",
            self.id
        );
        crate::rt::spawn_local(future)
    }
}

// ── Worker identity (sendable) ───────────────────────────────────────────────

/// Which plane a thread belongs to. See ADR 0001
/// (`docs/adr/0001-worker-scopes-and-planes.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkerRole {
    /// A sharded HTTP worker: `current_thread` runtime + `LocalSet`, owns one
    /// `SO_REUSEPORT` listener and the per-worker services.
    DataPlane,
    /// The caller's multi-thread runtime: boot, scheduler, consumers,
    /// executor, QUIC, shutdown. Also every thread of a non-sharded app.
    ControlPlane,
}

impl WorkerRole {
    /// Stable lowercase label (`"data"` / `"control"`) for metrics and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            WorkerRole::DataPlane => "data",
            WorkerRole::ControlPlane => "control",
        }
    }
}

impl std::fmt::Display for WorkerRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Identity of the worker the current code runs on — the sendable half of
/// [`WorkerContext`].
///
/// `Copy + Send + Sync`: pass it around, put it in log fields, hash it. It
/// grants nothing (no `spawn_local`, no socket adoption) — that stays on the
/// `!Send` [`WorkerContext`].
///
/// Obtain it from:
/// - [`WorkerContext::info`] inside a per-worker service;
/// - [`WorkerInfo::current`] anywhere (`None` on the control plane);
/// - a route parameter or `#[inject(request)]` field — it is an infallible
///   request extractor: a request served by worker `i` sees `id == i`; a
///   request served by the single-listener path sees the control plane.
///
/// `cpu` is the *effective* CPU affinity: `None` until R2E pins worker
/// threads (it does not today). Readers should treat `None` as "unknown",
/// never as CPU 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorkerInfo {
    id: usize,
    workers: usize,
    cpu: Option<usize>,
    role: WorkerRole,
}

thread_local! {
    static CURRENT_WORKER: Cell<Option<WorkerInfo>> = const { Cell::new(None) };
}

/// Number of data-plane workers last installed, for
/// [`WorkerInfo::control_plane`] to report a meaningful `workers`.
static INSTALLED_WORKERS: AtomicUsize = AtomicUsize::new(0);

impl WorkerInfo {
    /// Build an identity. Framework/harness use; applications read identities,
    /// they do not mint them.
    #[doc(hidden)]
    pub fn new(id: usize, workers: usize, cpu: Option<usize>, role: WorkerRole) -> Self {
        Self {
            id,
            workers,
            cpu,
            role,
        }
    }

    /// The identity of the control plane: `role == ControlPlane`, `id == 0`,
    /// `workers` = the number of data-plane workers currently installed in
    /// this process (`0` for a non-sharded app).
    pub fn control_plane() -> Self {
        Self {
            id: 0,
            workers: INSTALLED_WORKERS.load(Ordering::Relaxed),
            cpu: None,
            role: WorkerRole::ControlPlane,
        }
    }

    /// The worker the calling thread belongs to, or `None` when called from
    /// the control plane (or any thread that is not a sharded worker).
    pub fn current() -> Option<Self> {
        CURRENT_WORKER.with(|c| c.get())
    }

    /// Like [`current`](Self::current) but never `None`: falls back to
    /// [`control_plane`](Self::control_plane).
    pub fn current_or_control_plane() -> Self {
        Self::current().unwrap_or_else(Self::control_plane)
    }

    /// Record `self` as the calling thread's worker identity. Called by the
    /// worker bootstrap (sharded serving and the test harness) before the
    /// worker runtime is built.
    #[doc(hidden)]
    pub fn install(self) {
        INSTALLED_WORKERS.fetch_max(self.workers, Ordering::Relaxed);
        CURRENT_WORKER.with(|c| c.set(Some(self)));
    }

    /// Forget the calling thread's worker identity (worker thread exit).
    #[doc(hidden)]
    pub fn uninstall() {
        CURRENT_WORKER.with(|c| c.set(None));
    }

    /// Zero-based worker index (`0` on the control plane).
    pub fn id(&self) -> usize {
        self.id
    }

    /// Number of data-plane workers in this server.
    pub fn workers(&self) -> usize {
        self.workers
    }

    /// Effective CPU affinity, when known.
    pub fn cpu(&self) -> Option<usize> {
        self.cpu
    }

    /// Which plane this identity belongs to.
    pub fn role(&self) -> WorkerRole {
        self.role
    }

    /// `true` for a sharded worker, `false` for the control plane.
    pub fn is_data_plane(&self) -> bool {
        self.role == WorkerRole::DataPlane
    }
}

impl std::fmt::Display for WorkerInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.role {
            WorkerRole::DataPlane => write!(f, "worker {}/{}", self.id, self.workers),
            WorkerRole::ControlPlane => f.write_str("control-plane"),
        }
    }
}

// Named bridge point (plan §5.3b): route-method parameters and
// `#[inject(request)]` fields are extracted by the HTTP backend, reached
// through the `ViaAxum` blanket bridge of `FromRequestPartsVia`. Infallible:
// the identity is a thread-local read.
impl<S: Send + Sync> crate::http::extract::FromRequestParts<S> for WorkerInfo {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        _parts: &mut crate::http::header::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(WorkerInfo::current_or_control_plane())
    }
}

// ── Shared worker bootstrap helpers ──────────────────────────────────────────

/// Start `factories` in order for `ctx`'s worker. On failure, the services
/// already started are shut down (reverse order) and the failing factory's
/// index and error are returned. Shared by sharded serving and the test
/// harness so both follow the same all-or-nothing rule.
#[doc(hidden)]
pub async fn start_services(
    ctx: &WorkerContext,
    factories: &[PerWorkerServiceFactory],
) -> Result<Vec<Box<dyn WorkerService>>, (usize, BoxError)> {
    let mut started: Vec<Box<dyn WorkerService>> = Vec::with_capacity(factories.len());
    for (k, factory) in factories.iter().enumerate() {
        match factory.build(ctx.clone()).await {
            Ok(svc) => started.push(svc),
            Err(e) => {
                shutdown_services(ctx.id(), started).await;
                return Err((k, e));
            }
        }
    }
    Ok(started)
}

/// Shut down `services` in reverse start order, awaiting each cleanup.
#[doc(hidden)]
pub async fn shutdown_services(worker: usize, mut services: Vec<Box<dyn WorkerService>>) {
    while let Some(svc) = services.pop() {
        let idx = services.len();
        tracing::debug!(worker, service = idx, "shutting down per-worker service");
        svc.shutdown().await;
    }
}
