//! `WorkerHarness` — real sharded workers without HTTP, for tests.
//!
//! Spawns `n` worker threads exactly like sharded serving does — one
//! `current_thread` runtime + `LocalSet` each, [`WorkerInfo`] installed,
//! control-plane handle registered, per-worker service factories run behind
//! the same all-or-nothing barrier — but instead of an HTTP listener each
//! worker executes the closures you send it with [`WorkerHarness::run_on`].
//! The harness owns a [`WorkerSet`] whose states/counters move exactly as in
//! production, so a test can assert:
//!
//! - **instance count** — `WorkerLocal::instances() == n` after start, `0`
//!   after shutdown;
//! - **execution worker** — `run_on(i, |ctx| ..)` observes `ctx.id() == i`
//!   and `WorkerInfo::current()`;
//! - **routing** — `Mailboxes` deliveries land on the intended worker, and
//!   local/remote crossings are what the design says;
//! - **drain** — services shut down in reverse order, on the worker thread,
//!   before the worker exits.
//!
//! ```ignore
//! let hits: WorkerLocal<Cell<u64>> = WorkerLocal::new(|_| async { Ok(Cell::new(0)) });
//! let h = WorkerHarness::start(3, vec![hits.clone().into_factory()]).await?;
//! assert_eq!(hits.instances(), 3);
//! let (id, v) = h.run_on(1, { let hits = hits.clone(); move |ctx| async move {
//!     hits.with(|c| c.set(c.get() + 1));
//!     (ctx.id(), hits.with(|c| c.get()))
//! }}).await;
//! assert_eq!((id, v), (1, 1));
//! h.shutdown().await;
//! assert_eq!(hits.instances(), 0);
//! ```

use std::future::Future;
use std::sync::Arc;

use super::worker::{
    shutdown_services, start_services, LocalBoxFuture, PerWorkerServiceFactory, WorkerContext,
    WorkerInfo, WorkerRole,
};
use super::worker_local::WorkerLocal;
use super::worker_set::{WorkerSet, WorkerState};
use crate::rt::sync::{mpsc, oneshot};
use crate::rt::CancelToken;

type Job = Box<dyn FnOnce(WorkerContext) -> LocalBoxFuture<'static, ()> + Send>;

/// `n` live sharded workers driven by a test. See the [module docs](self).
pub struct WorkerHarness {
    set: WorkerSet,
    workers: usize,
    jobs: Vec<mpsc::UnboundedSender<Job>>,
    shutdown: CancelToken,
    threads: Vec<std::thread::JoinHandle<()>>,
    /// Owned control-plane runtime when the harness was started outside one.
    _control: Option<crate::rt::Runtime>,
}

impl std::fmt::Debug for WorkerHarness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerHarness")
            .field("workers", &self.workers)
            .field("states", &self.set.states())
            .finish()
    }
}

impl WorkerHarness {
    /// Spawn `workers` worker threads and run every factory on each, in
    /// order, behind the all-or-nothing barrier. Resolves once every worker
    /// is [`WorkerState::Serving`], or with the first startup error (after
    /// every worker has unwound and exited).
    pub async fn start(
        workers: usize,
        factories: Vec<PerWorkerServiceFactory>,
    ) -> Result<Self, String> {
        assert!(workers >= 1, "WorkerHarness needs at least one worker");
        let set = WorkerSet::new();
        set.configure(workers);
        let factories: Arc<[PerWorkerServiceFactory]> = factories.into();

        let (control_plane, _control) = match crate::rt::RuntimeHandle::try_current() {
            Some(h) => (h, None),
            None => {
                let rt = crate::rt::RuntimeBuilder::new_multi_thread()
                    .enable_all()
                    .worker_threads(2)
                    .thread_name("r2e-harness-ctl")
                    .build()
                    .map_err(|e| format!("harness control plane: {e}"))?;
                (rt.handle(), Some(rt))
            }
        };

        let shutdown = CancelToken::new();
        let start_gate = CancelToken::new();
        let mut jobs = Vec::with_capacity(workers);
        let mut threads = Vec::with_capacity(workers);
        let mut reports = Vec::with_capacity(workers);

        for i in 0..workers {
            let (job_tx, mut job_rx) = mpsc::unbounded_channel::<Job>();
            let (report_tx, report_rx) = oneshot::channel::<Result<(), String>>();
            jobs.push(job_tx);
            reports.push(report_rx);
            let slot = set.slot(i).expect("slot configured");
            let factories = Arc::clone(&factories);
            let child = shutdown.child_token();
            let gate = start_gate.clone();
            let control_plane = control_plane.clone();

            let handle = std::thread::Builder::new()
                .name(format!("r2e-worker-{i}"))
                .spawn(move || {
                    WorkerInfo::new(i, workers, None, WorkerRole::DataPlane).install();
                    crate::rt::set_control_plane(control_plane);
                    slot.set_state(WorkerState::Starting);
                    let rt = match crate::rt::RuntimeBuilder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        Ok(rt) => rt,
                        Err(e) => {
                            let msg = format!("worker {i}: failed to build worker runtime: {e}");
                            slot.fail(msg.clone());
                            let _ = report_tx.send(Err(msg));
                            return;
                        }
                    };
                    let local = crate::rt::LocalSet::new();
                    let slot2 = Arc::clone(&slot);
                    rt.block_on(local.run_until(async move {
                        let ctx = WorkerContext::new(i, workers, None, child.clone());
                        let services = match start_services(&ctx, &factories).await {
                            Ok(s) => s,
                            Err((k, e)) => {
                                let msg = format!(
                                    "worker {i}: per-worker service #{k} failed to start: {e}"
                                );
                                slot2.fail(msg.clone());
                                let _ = report_tx.send(Err(msg));
                                return;
                            }
                        };
                        slot2.set_state(WorkerState::Ready);
                        let _ = report_tx.send(Ok(()));
                        let released = crate::rt::select! {
                            _ = gate.cancelled() => true,
                            _ = child.cancelled() => false,
                        };
                        if !released {
                            shutdown_services(i, services).await;
                            slot2.set_state(WorkerState::Exited);
                            return;
                        }
                        slot2.set_state(WorkerState::Serving);
                        loop {
                            crate::rt::select! {
                                job = job_rx.recv() => match job {
                                    Some(job) => { crate::rt::spawn_local(job(ctx.clone())); }
                                    None => break,
                                },
                                _ = child.cancelled() => break,
                            }
                        }
                        slot2.set_state(WorkerState::Draining);
                        child.cancel();
                        shutdown_services(i, services).await;
                        slot2.set_state(WorkerState::ServicesDown);
                    }));
                    drop(local);
                    drop(rt);
                    if slot.state() != WorkerState::Failed {
                        slot.set_state(WorkerState::Exited);
                    }
                    WorkerInfo::uninstall();
                })
                .map_err(|e| format!("failed to spawn worker thread {i}: {e}"))?;
            threads.push(handle);
        }

        let mut first_err = None;
        for (i, rx) in reports.into_iter().enumerate() {
            match rx.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    first_err.get_or_insert(e);
                }
                Err(_) => {
                    first_err.get_or_insert(format!("worker {i} vanished during startup"));
                }
            }
        }
        let harness = Self {
            set,
            workers,
            jobs,
            shutdown,
            threads,
            _control,
        };
        match first_err {
            None => {
                start_gate.cancel();
                harness.set.wait_all_serving().await;
                Ok(harness)
            }
            Some(e) => {
                harness.shutdown().await;
                Err(e)
            }
        }
    }

    /// Number of workers.
    pub fn workers(&self) -> usize {
        self.workers
    }

    /// The live [`WorkerSet`] (states, crossings, mailbox counters).
    pub fn worker_set(&self) -> &WorkerSet {
        &self.set
    }

    /// The shared shutdown token (each worker holds a child of it).
    pub fn shutdown_token(&self) -> CancelToken {
        self.shutdown.clone()
    }

    /// Run `f` on worker `worker`'s thread, inside its `LocalSet`, and return
    /// its result. `f` runs where a per-worker service runs: `WorkerLocal`
    /// slots, `spawn_local`, adopted sockets are all reachable; the future
    /// need not be `Send`, only the result must be.
    ///
    /// # Panics
    ///
    /// If `worker >= workers()` or the worker has already shut down.
    pub async fn run_on<F, Fut, R>(&self, worker: usize, f: F) -> R
    where
        F: FnOnce(WorkerContext) -> Fut + Send + 'static,
        Fut: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        let tx = self
            .jobs
            .get(worker)
            .unwrap_or_else(|| panic!("WorkerHarness has no worker {worker}"));
        let (reply_tx, reply_rx) = oneshot::channel();
        let job: Job = Box::new(move |ctx| {
            Box::pin(async move {
                let r = f(ctx).await;
                let _ = reply_tx.send(r);
            })
        });
        tx.send(job)
            .unwrap_or_else(|_| panic!("worker {worker} is no longer accepting jobs"));
        reply_rx
            .await
            .unwrap_or_else(|_| panic!("worker {worker} dropped the job before completing it"))
    }

    /// [`run_on`](Self::run_on) every worker in index order.
    pub async fn run_on_all<F, Fut, R>(&self, f: F) -> Vec<R>
    where
        F: Fn(WorkerContext) -> Fut + Clone + Send + 'static,
        Fut: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        let mut out = Vec::with_capacity(self.workers);
        for i in 0..self.workers {
            out.push(self.run_on(i, f.clone()).await);
        }
        out
    }

    /// Cancel every worker, wait for their services to shut down (reverse
    /// order, on the worker) and join the threads. The [`WorkerSet`] ends
    /// with every worker `Exited` (or `Failed`).
    pub async fn shutdown(mut self) {
        self.shutdown.cancel();
        let threads = std::mem::take(&mut self.threads);
        let set = self.set.clone();
        crate::rt::spawn_blocking(move || {
            for t in threads {
                let _ = t.join();
            }
        })
        .await
        .ok();
        set.wait_all_exited().await;
    }
}

impl Drop for WorkerHarness {
    fn drop(&mut self) {
        self.shutdown.cancel();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

impl<T: 'static> WorkerLocal<T> {
    /// The per-worker service factory that installs this slot — what
    /// [`AppBuilder::worker_local`](crate::builder::AppBuilder::worker_local)
    /// registers, exposed for [`WorkerHarness::start`].
    pub fn into_factory(self) -> PerWorkerServiceFactory {
        PerWorkerServiceFactory::new(move |ctx| {
            let local = self.clone();
            async move { local.install(ctx).await }
        })
    }
}
