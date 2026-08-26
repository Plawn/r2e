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
    inner: Arc<
        dyn Fn(WorkerContext) -> LocalBoxFuture<'static, Result<Box<dyn WorkerService>, BoxError>>
            + Send
            + Sync,
    >,
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
                Box::pin(async move {
                    fut.await
                        .map(|s| Box::new(s) as Box<dyn WorkerService>)
                })
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
