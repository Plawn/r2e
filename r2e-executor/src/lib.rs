//! Managed task pool executor for R2E.
//!
//! Provides a bounded, configurable, injectable Tokio task pool, à la J2EE
//! `ManagedExecutorService`. Install with `.plugin(Executor)` before
//! `build_state()` to make `PoolExecutor` available for `#[inject]`.
//!
//! # Example
//!
//! ```ignore
//! use r2e_core::AppBuilder;
//! use r2e_executor::{Executor, PoolExecutor};
//!
//! AppBuilder::new()
//!     .plugin(Executor)
//!     .build_state::<Services, _, _>()
//!     .await
//!     .register_controller::<MyController>()
//!     .serve("0.0.0.0:3000")
//!     .await
//! ```
//!
//! Inject and submit work:
//!
//! ```ignore
//! #[derive(Controller)]
//! #[controller(state = Services)]
//! pub struct ReportController {
//!     #[inject] executor: PoolExecutor,
//! }
//!
//! #[routes]
//! impl ReportController {
//!     #[post("/reports")]
//!     async fn create(&self) -> Json<&'static str> {
//!         self.executor.submit_detached(async move {
//!             // long-running background work
//!         });
//!         Json("queued")
//!     }
//! }
//! ```

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::sync::{oneshot, Notify, Semaphore};
use tokio_util::sync::CancellationToken;

use r2e_core::config::ConfigProperties;
use r2e_core::plugin::{DeferredAction, PluginInstallContext, PreStatePlugin};

// ── Configuration ─────────────────────────────────────────────────────────

/// Configuration for the [`PoolExecutor`].
///
/// Reads from the `executor.*` section of `R2eConfig`.
///
/// # YAML example
///
/// ```yaml
/// executor:
///   max-concurrent: 32
///   queue-capacity: 1024
///   shutdown-timeout-secs: 30
/// ```
#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
pub struct ExecutorConfig {
    /// Maximum number of jobs running concurrently. Acts as the semaphore size.
    #[config(key = "max-concurrent", default = 32)]
    pub max_concurrent: i64,

    /// Maximum number of jobs that can be queued waiting for a permit.
    /// `try_submit` rejects when `queued + running >= max_concurrent + queue_capacity`.
    #[config(key = "queue-capacity", default = 1024)]
    pub queue_capacity: i64,

    /// How long [`PoolExecutor::shutdown_graceful`] waits for in-flight jobs to finish.
    #[config(key = "shutdown-timeout-secs", default = 30)]
    pub shutdown_timeout_secs: i64,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 32,
            queue_capacity: 1024,
            shutdown_timeout_secs: 30,
        }
    }
}

// ── Errors ────────────────────────────────────────────────────────────────

/// Reason a submission was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectedError {
    /// The pool is at capacity (`queued + running >= max_concurrent + queue_capacity`).
    QueueFull,
    /// The pool has been shut down and is no longer accepting work.
    Shutdown,
}

impl std::fmt::Display for RejectedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::QueueFull => write!(f, "executor queue is full"),
            Self::Shutdown => write!(f, "executor is shut down"),
        }
    }
}

impl std::error::Error for RejectedError {}

/// Reason a job did not yield a result.
#[derive(Debug)]
pub enum JobError {
    /// The pool was shut down before the job could run.
    Shutdown,
    /// The job's worker task panicked or was aborted.
    Cancelled,
}

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shutdown => write!(f, "job did not run: executor shut down"),
            Self::Cancelled => write!(f, "job was cancelled or panicked"),
        }
    }
}

impl std::error::Error for JobError {}

// ── Metrics ───────────────────────────────────────────────────────────────

/// Snapshot of executor counters at a point in time.
#[derive(Debug, Clone, Copy, Default)]
pub struct ExecutorMetrics {
    /// Number of jobs currently waiting for a permit.
    pub queued: u64,
    /// Number of jobs currently executing (holding a permit).
    pub running: u64,
    /// Cumulative count of jobs that completed (successfully or with panic).
    pub completed: u64,
    /// Cumulative count of submissions refused by `try_submit`.
    pub rejected: u64,
}

// ── PoolExecutor ──────────────────────────────────────────────────────────

struct Inner {
    semaphore: Arc<Semaphore>,
    /// Total in-flight cap = max_concurrent + queue_capacity. Used by `try_submit`.
    cap: u64,
    shutdown: CancellationToken,
    queued: AtomicU64,
    running: AtomicU64,
    completed: AtomicU64,
    rejected: AtomicU64,
    notify_idle: Notify,
    shutdown_timeout: Duration,
}

/// Cloneable handle to a managed Tokio task pool.
///
/// All clones share the same backing pool, semaphore, and metrics.
#[derive(Clone)]
pub struct PoolExecutor {
    inner: Arc<Inner>,
}

impl PoolExecutor {
    /// Build a [`PoolExecutor`] from an [`ExecutorConfig`].
    pub fn new(config: ExecutorConfig) -> Self {
        let max_concurrent = config.max_concurrent.max(1) as usize;
        let queue_capacity = config.queue_capacity.max(0) as u64;
        let shutdown_timeout = Duration::from_secs(config.shutdown_timeout_secs.max(0) as u64);
        Self {
            inner: Arc::new(Inner {
                semaphore: Arc::new(Semaphore::new(max_concurrent)),
                cap: max_concurrent as u64 + queue_capacity,
                shutdown: CancellationToken::new(),
                queued: AtomicU64::new(0),
                running: AtomicU64::new(0),
                completed: AtomicU64::new(0),
                rejected: AtomicU64::new(0),
                notify_idle: Notify::new(),
                shutdown_timeout,
            }),
        }
    }

    /// Submit a future to the pool. Returns a [`JobHandle`] that resolves to the result.
    ///
    /// This call always queues — use [`try_submit`](Self::try_submit) to apply backpressure.
    pub fn submit<F, T>(&self, fut: F) -> JobHandle<T>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        if self.inner.shutdown.is_cancelled() {
            self.inner.rejected.fetch_add(1, Ordering::Relaxed);
            let _ = tx.send(Err(JobError::Shutdown));
            return JobHandle { rx };
        }
        self.spawn_job(fut, tx);
        JobHandle { rx }
    }

    /// Submit a fire-and-forget future. No handle is returned.
    pub fn submit_detached<F>(&self, fut: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        if self.inner.shutdown.is_cancelled() {
            self.inner.rejected.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let permit = match inner.semaphore.clone().try_acquire_owned() {
                Ok(p) => p,
                Err(tokio::sync::TryAcquireError::NoPermits) => {
                    inner.queued.fetch_add(1, Ordering::Relaxed);
                    let p = match inner.semaphore.clone().acquire_owned().await {
                        Ok(p) => p,
                        Err(_) => {
                            inner.queued.fetch_sub(1, Ordering::Relaxed);
                            return;
                        }
                    };
                    inner.queued.fetch_sub(1, Ordering::Relaxed);
                    p
                }
                Err(tokio::sync::TryAcquireError::Closed) => return,
            };
            inner.running.fetch_add(1, Ordering::Relaxed);
            fut.await;
            drop(permit);
            let prev = inner.running.fetch_sub(1, Ordering::Relaxed);
            inner.completed.fetch_add(1, Ordering::Relaxed);
            if prev == 1 && inner.shutdown.is_cancelled() {
                inner.notify_idle.notify_waiters();
            }
        });
    }

    /// Try to submit a future, applying queue-depth backpressure.
    ///
    /// Returns [`RejectedError::QueueFull`] when the in-flight count would exceed
    /// `max_concurrent + queue_capacity`, or [`RejectedError::Shutdown`] when the
    /// pool has been shut down.
    pub fn try_submit<F, T>(&self, fut: F) -> Result<JobHandle<T>, RejectedError>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        if self.inner.shutdown.is_cancelled() {
            self.inner.rejected.fetch_add(1, Ordering::Relaxed);
            return Err(RejectedError::Shutdown);
        }
        let queued = self.inner.queued.load(Ordering::Relaxed);
        let running = self.inner.running.load(Ordering::Relaxed);
        if queued + running >= self.inner.cap {
            self.inner.rejected.fetch_add(1, Ordering::Relaxed);
            return Err(RejectedError::QueueFull);
        }
        let (tx, rx) = oneshot::channel();
        self.spawn_job(fut, tx);
        Ok(JobHandle { rx })
    }

    fn spawn_job<F, T>(&self, fut: F, tx: oneshot::Sender<Result<T, JobError>>)
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let inner = self.inner.clone();
        let shutdown = inner.shutdown.clone();
        tokio::spawn(async move {
            let permit = match inner.semaphore.clone().try_acquire_owned() {
                Ok(p) => p,
                Err(tokio::sync::TryAcquireError::Closed) => {
                    let _ = tx.send(Err(JobError::Shutdown));
                    return;
                }
                Err(tokio::sync::TryAcquireError::NoPermits) => {
                    inner.queued.fetch_add(1, Ordering::Relaxed);
                    let p = tokio::select! {
                        biased;
                        _ = shutdown.cancelled() => {
                            inner.queued.fetch_sub(1, Ordering::Relaxed);
                            let _ = tx.send(Err(JobError::Shutdown));
                            return;
                        }
                        p = inner.semaphore.clone().acquire_owned() => match p {
                            Ok(p) => p,
                            Err(_) => {
                                inner.queued.fetch_sub(1, Ordering::Relaxed);
                                let _ = tx.send(Err(JobError::Shutdown));
                                return;
                            }
                        }
                    };
                    inner.queued.fetch_sub(1, Ordering::Relaxed);
                    p
                }
            };
            inner.running.fetch_add(1, Ordering::Relaxed);
            let result = fut.await;
            drop(permit);
            let prev = inner.running.fetch_sub(1, Ordering::Relaxed);
            inner.completed.fetch_add(1, Ordering::Relaxed);
            if prev == 1 && inner.shutdown.is_cancelled() {
                inner.notify_idle.notify_waiters();
            }
            let _ = tx.send(Ok(result));
        });
    }

    /// Read a snapshot of the executor's metrics.
    pub fn metrics(&self) -> ExecutorMetrics {
        ExecutorMetrics {
            queued: self.inner.queued.load(Ordering::Relaxed),
            running: self.inner.running.load(Ordering::Relaxed),
            completed: self.inner.completed.load(Ordering::Relaxed),
            rejected: self.inner.rejected.load(Ordering::Relaxed),
        }
    }

    /// True once [`shutdown`](Self::shutdown) has been called.
    pub fn is_shut_down(&self) -> bool {
        self.inner.shutdown.is_cancelled()
    }

    /// Refuse new work and signal pending acquires to abandon. In-flight jobs continue.
    pub fn shutdown(&self) {
        self.inner.shutdown.cancel();
        self.inner.semaphore.close();
    }

    /// Configured graceful-shutdown timeout (from [`ExecutorConfig::shutdown_timeout_secs`]).
    pub(crate) fn shutdown_timeout(&self) -> Duration {
        self.inner.shutdown_timeout
    }

    /// Cancel new submissions and await running jobs to finish, bounded by `timeout`.
    ///
    /// Returns `true` when the pool drained cleanly, `false` when the timeout elapsed
    /// while jobs were still running.
    pub async fn shutdown_graceful(&self, timeout: Duration) -> bool {
        self.shutdown();
        let drain = async {
            loop {
                // Register the waiter first; if running drops to 0 between the
                // load and the await, the notification would otherwise be lost.
                let waiter = self.inner.notify_idle.notified();
                if self.inner.running.load(Ordering::Acquire) == 0 {
                    return;
                }
                waiter.await;
            }
        };
        tokio::time::timeout(timeout, drain).await.is_ok()
    }
}

// ── JobHandle ─────────────────────────────────────────────────────────────

/// Handle to an in-flight job. `await` resolves to the job's result.
pub struct JobHandle<T> {
    rx: oneshot::Receiver<Result<T, JobError>>,
}

impl<T> JobHandle<T> {
    /// Await the job's result. Equivalent to `.await`.
    pub async fn await_result(self) -> Result<T, JobError> {
        self.await
    }
}

impl<T> Future for JobHandle<T> {
    type Output = Result<T, JobError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.rx).poll(cx) {
            Poll::Ready(Ok(res)) => Poll::Ready(res),
            Poll::Ready(Err(_)) => Poll::Ready(Err(JobError::Cancelled)),
            Poll::Pending => Poll::Pending,
        }
    }
}

// ── Plugin ────────────────────────────────────────────────────────────────

/// Plugin that builds a [`PoolExecutor`] from `R2eConfig` and provides it as a bean.
///
/// Reads the `executor.*` section. Falls back to [`ExecutorConfig::default`] when
/// no config is loaded or the section is absent.
///
/// Install with `.plugin(Executor)` **before** `build_state()`.
pub struct Executor;

impl PreStatePlugin for Executor {
    type Provided = PoolExecutor;
    type Deps = ();

    fn install(self, _deps: (), ctx: &mut PluginInstallContext<'_>) -> PoolExecutor {
        let config = ctx
            .config()
            .map(|c| ExecutorConfig::from_config(c, Some("executor")))
            .transpose()
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "Invalid executor config; using defaults");
                None
            })
            .unwrap_or_default();

        let executor = PoolExecutor::new(config);
        let shutdown_handle = executor.clone();

        ctx.add_deferred(DeferredAction::new("Executor", move |dctx| {
            dctx.on_shutdown(move || {
                let timeout = shutdown_handle.shutdown_timeout();
                shutdown_handle.shutdown();
                if timeout.is_zero() {
                    return;
                }
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    let to_drain = shutdown_handle.clone();
                    handle.spawn(async move {
                        let _ = to_drain.shutdown_graceful(timeout).await;
                    });
                }
            });
        }));

        executor
    }
}
