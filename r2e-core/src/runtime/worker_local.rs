//! `WorkerLocal<T>` — the worker-local injection scope.
//!
//! One instance of `T` per sharded worker, built on that worker's thread
//! (inside its `LocalSet`, before it accepts a connection) and dropped on that
//! same thread after HTTP drain. The **handle** is `Clone + Send + Sync` and
//! lives in the bean graph like any singleton; the **value** is reachable only
//! through [`WorkerLocal::with`], which resolves against the calling thread's
//! slot. See ADR 0001 (`docs/adr/0001-worker-scopes-and-planes.md`).
//!
//! ```ignore
//! #[derive(Default)]
//! struct Hits(Cell<u64>);            // !Sync — fine, it never leaves the worker
//!
//! AppBuilder::new()
//!     .worker_local(|worker: WorkerContext| async move {
//!         tracing::info!(worker = worker.id(), "building shard-local Hits");
//!         Ok::<_, BoxError>(Hits::default())
//!     })
//!     // … `WorkerLocal<Hits>` is now a bean:
//!
//! #[controller(path = "/")]
//! struct Api { #[inject] hits: WorkerLocal<Hits> }
//!
//! #[routes]
//! impl Api {
//!     #[get("/hit")]
//!     async fn hit(&self) -> String {
//!         self.hits.with(|h| { h.0.set(h.0.get() + 1); h.0.get() }).to_string()
//!     }
//! }
//! ```
//!
//! # Type contract
//!
//! `T: 'static` — **not** `Send`, **not** `Sync`. `Rc`, `RefCell`, `Cell`,
//! adopted sockets and `spawn_local` task handles are all valid. The factory
//! is shared by every worker, so it is `Send + Sync + 'static` (capture `Arc`s
//! and config); the future it returns and the value it resolves to need not be.
//!
//! # Thread contract
//!
//! | Event | Thread |
//! |---|---|
//! | factory runs | worker `i`, inside its `LocalSet`, before serving |
//! | `with(..)` | any thread — **must** be worker `i` to see worker `i`'s value |
//! | drop | worker `i`, after HTTP drain and the services started after it (reverse order) |
//!
//! `with` on a thread that has no instance (the control plane, a
//! `TestApp::boot` handler, another worker's thread) **panics** with the slot
//! name and the current [`WorkerInfo`]; use [`WorkerLocal::try_with`] where
//! absence is a legitimate state. It never falls through to a shared
//! instance — that is what makes the scope impossible to mistake for an app
//! singleton.
//!
//! # Not sharded?
//!
//! `worker_local` registers a per-worker service, so an app without
//! `server.workers` fails at `run()` with
//! [`PER_WORKER_REQUIRES_SHARDING_MSG`](crate::builder::PER_WORKER_REQUIRES_SHARDING_MSG).
//! There is no worker to own the value; the single-listener behaviour is
//! unchanged.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use super::worker::{BoxError, LocalBoxFuture, WorkerContext, WorkerInfo, WorkerService};

thread_local! {
    /// This thread's worker-local instances, keyed by slot.
    static SLOTS: RefCell<HashMap<usize, Box<dyn Any>>> = RefCell::new(HashMap::new());
}

/// Process-wide slot allocator: every `WorkerLocal::new` gets a fresh key.
static NEXT_SLOT: AtomicUsize = AtomicUsize::new(1);

/// Lifetime counters of a slot, across all workers.
#[derive(Default)]
struct Stats {
    built: AtomicUsize,
    dropped: AtomicUsize,
}

/// Type-erased worker-local factory.
trait LocalFactory<T>:
    Fn(WorkerContext) -> LocalBoxFuture<'static, Result<T, BoxError>> + Send + Sync
{
}
impl<T, F> LocalFactory<T> for F where
    F: Fn(WorkerContext) -> LocalBoxFuture<'static, Result<T, BoxError>> + Send + Sync
{
}

/// Handle to a worker-local value: exactly one `T` per sharded worker.
///
/// See the [module docs](self). Obtained through
/// [`AppBuilder::worker_local`](crate::builder::AppBuilder::worker_local)
/// (which also registers the per-worker service that builds the value) and
/// injected with `#[inject]`.
pub struct WorkerLocal<T: 'static> {
    key: usize,
    name: &'static str,
    factory: Arc<dyn LocalFactory<T>>,
    stats: Arc<Stats>,
    /// `fn() -> T` keeps the handle `Send + Sync` whatever `T` is: the handle
    /// never carries a `T`, it only knows how to build one on a worker.
    _marker: PhantomData<fn() -> T>,
}

impl<T: 'static> Clone for WorkerLocal<T> {
    fn clone(&self) -> Self {
        Self {
            key: self.key,
            name: self.name,
            factory: Arc::clone(&self.factory),
            stats: Arc::clone(&self.stats),
            _marker: PhantomData,
        }
    }
}

impl<T: 'static> std::fmt::Debug for WorkerLocal<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerLocal")
            .field("type", &self.name)
            .field("slot", &self.key)
            .field("instances", &self.instances())
            .finish()
    }
}

impl<T: 'static> WorkerLocal<T> {
    /// Create a handle whose value worker `i` builds by running `factory`
    /// once on its own thread.
    ///
    /// Prefer [`AppBuilder::worker_local`](crate::builder::AppBuilder::worker_local),
    /// which also registers the per-worker service; use this directly only
    /// with the test harness or a hand-rolled `per_worker_service` calling
    /// [`install`](Self::install).
    pub fn new<F, Fut>(factory: F) -> Self
    where
        F: Fn(WorkerContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<T, BoxError>> + 'static,
    {
        Self {
            key: NEXT_SLOT.fetch_add(1, Ordering::Relaxed),
            name: std::any::type_name::<T>(),
            factory: Arc::new(move |ctx| Box::pin(factory(ctx)) as LocalBoxFuture<'static, _>),
            stats: Arc::new(Stats::default()),
            _marker: PhantomData,
        }
    }

    /// Build this worker's instance and store it in the calling thread's
    /// slot. Must run on a worker thread (inside its `LocalSet`); the
    /// returned guard is the [`WorkerService`] whose shutdown drops the
    /// instance on that same thread.
    ///
    /// Fails if this thread already holds an instance for the slot (a
    /// `WorkerLocal` installed twice on one worker).
    pub async fn install(&self, ctx: WorkerContext) -> Result<WorkerLocalGuard<T>, BoxError> {
        let already = SLOTS.with(|s| s.borrow().contains_key(&self.key));
        if already {
            return Err(format!(
                "WorkerLocal<{}> is already installed on worker {}",
                self.name,
                ctx.id()
            )
            .into());
        }
        let value = (self.factory)(ctx.clone()).await?;
        SLOTS.with(|s| s.borrow_mut().insert(self.key, Box::new(value)));
        self.stats.built.fetch_add(1, Ordering::Relaxed);
        Ok(WorkerLocalGuard {
            key: self.key,
            name: self.name,
            worker: ctx.id(),
            stats: Arc::clone(&self.stats),
            _marker: PhantomData,
        })
    }

    /// Run `f` against the calling worker's instance.
    ///
    /// # Panics
    ///
    /// When the calling thread holds no instance for this slot — the control
    /// plane, a non-sharded app, or a worker whose services are already down.
    /// The message names the type and the current [`WorkerInfo`].
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        match self.try_with(f) {
            Some(r) => r,
            None => panic!(
                "WorkerLocal<{}>::with called on {} which holds no instance: worker-local \
                 values are only reachable on the worker that built them (thread {:?})",
                self.name,
                WorkerInfo::current_or_control_plane(),
                std::thread::current().name().unwrap_or("?"),
            ),
        }
    }

    /// Run `f` against the calling worker's instance, or `None` when this
    /// thread holds none.
    ///
    /// Do not call [`install`](Self::install) from inside `f` — the slot map
    /// is borrowed for the duration of `f`.
    pub fn try_with<R>(&self, f: impl FnOnce(&T) -> R) -> Option<R> {
        SLOTS.with(|s| {
            let slots = s.borrow();
            let value = slots.get(&self.key)?;
            let value = value
                .downcast_ref::<T>()
                .expect("WorkerLocal slot holds a value of another type");
            Some(f(value))
        })
    }

    /// `true` when the calling thread holds an instance for this slot.
    pub fn is_installed(&self) -> bool {
        SLOTS.with(|s| s.borrow().contains_key(&self.key))
    }

    /// Instances currently alive across all workers (`built - dropped`).
    pub fn instances(&self) -> usize {
        self.stats
            .built
            .load(Ordering::Relaxed)
            .saturating_sub(self.stats.dropped.load(Ordering::Relaxed))
    }

    /// Instances ever built across all workers.
    pub fn built(&self) -> usize {
        self.stats.built.load(Ordering::Relaxed)
    }

    /// Instances dropped so far across all workers.
    pub fn dropped(&self) -> usize {
        self.stats.dropped.load(Ordering::Relaxed)
    }

    /// The type name of `T`, as used in diagnostics.
    pub fn type_name(&self) -> &'static str {
        self.name
    }
}

/// Owner of one worker's instance of a [`WorkerLocal`]. Returned by
/// [`WorkerLocal::install`]; a [`WorkerService`] whose shutdown drops the
/// instance, and whose plain `Drop` does the same as a safety net.
pub struct WorkerLocalGuard<T: 'static> {
    key: usize,
    name: &'static str,
    worker: usize,
    stats: Arc<Stats>,
    /// Pins the guard to the worker thread like the value it owns.
    _marker: PhantomData<std::cell::Cell<*const T>>,
}

impl<T: 'static> WorkerLocalGuard<T> {
    fn remove(&mut self) {
        let removed = SLOTS.with(|s| s.borrow_mut().remove(&self.key));
        if let Some(value) = removed {
            tracing::debug!(
                worker = self.worker,
                r#type = self.name,
                "dropping worker-local"
            );
            drop(value);
            self.stats.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl<T: 'static> WorkerService for WorkerLocalGuard<T> {
    fn shutdown(mut self: Box<Self>) -> LocalBoxFuture<'static, ()> {
        self.remove();
        Box::pin(async {})
    }
}

impl<T: 'static> Drop for WorkerLocalGuard<T> {
    fn drop(&mut self) {
        self.remove();
    }
}
