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
//!     .build_state()
//!     .await
//!     .register_controller::<MyController>()
//!     .serve("0.0.0.0:3000")
//!     .await
//! ```
//!
//! Inject and submit work:
//!
//! ```ignore
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
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::{Notify, Semaphore};
use tokio_util::sync::CancellationToken;

use r2e_core::config::ConfigProperties;
use r2e_core::plugin::{PluginInstallContext, PreStatePlugin};

pub use r2e_core::rt::{JobHandle, JoinError};

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
///   shutdown-timeout: 30s        # or: 30, "1m", "500ms"
/// ```
#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
pub struct ExecutorConfig {
    /// Maximum number of jobs running concurrently. Acts as the semaphore size.
    #[config(key = "max-concurrent", default = 32)]
    pub max_concurrent: u64,

    /// Maximum number of jobs that can be queued waiting for a permit.
    /// `try_submit` rejects when `queued + running >= max_concurrent + queue_capacity`.
    #[config(key = "queue-capacity", default = 1024)]
    pub queue_capacity: u64,

    /// How long [`PoolExecutor::shutdown_graceful`] waits for in-flight jobs to finish.
    /// Accepts an integer (seconds) or a duration string like `"30s"`, `"1m"`.
    #[config(key = "shutdown-timeout", default = std::time::Duration::from_secs(30))]
    pub shutdown_timeout: Duration,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 32,
            queue_capacity: 1024,
            shutdown_timeout: Duration::from_secs(30),
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
    max_concurrent: u64,
    /// Total in-flight cap = max_concurrent + queue_capacity. Used by `try_submit`.
    cap: u64,
    shutdown: CancellationToken,
    queued: AtomicU64,
    /// Jobs currently holding a permit. Used for the shutdown drain path
    /// (idle notification) — the running count for metrics is derived from the
    /// semaphore when the pool is open.
    drain_count: AtomicU64,
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
        Self {
            inner: Arc::new(Inner {
                semaphore: Arc::new(Semaphore::new(max_concurrent)),
                max_concurrent: max_concurrent as u64,
                cap: max_concurrent as u64 + config.queue_capacity,
                shutdown: CancellationToken::new(),
                queued: AtomicU64::new(0),
                drain_count: AtomicU64::new(0),
                completed: AtomicU64::new(0),
                rejected: AtomicU64::new(0),
                notify_idle: Notify::new(),
                shutdown_timeout: config.shutdown_timeout,
            }),
        }
    }

    /// Submit a future to the pool.
    ///
    /// Returns a [`JobHandle`] that resolves to the job's result.
    /// Returns [`RejectedError::Shutdown`] if the pool has been shut down.
    ///
    /// This call always queues — use [`try_submit`](Self::try_submit) to apply backpressure.
    pub fn submit<F, T>(&self, fut: F) -> Result<JobHandle<T>, RejectedError>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        if self.inner.shutdown.is_cancelled() {
            self.inner.rejected.fetch_add(1, Ordering::Relaxed);
            return Err(RejectedError::Shutdown);
        }
        Ok(self.spawn_job(fut))
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
        r2e_core::rt::spawn_ctl(async move {
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
            inner.drain_count.fetch_add(1, Ordering::Relaxed);
            fut.await;
            drop(permit);
            let prev = inner.drain_count.fetch_sub(1, Ordering::Relaxed);
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
        let running = self.inner.max_concurrent - self.inner.semaphore.available_permits() as u64;
        if queued + running >= self.inner.cap {
            self.inner.rejected.fetch_add(1, Ordering::Relaxed);
            return Err(RejectedError::QueueFull);
        }
        Ok(self.spawn_job(fut))
    }

    fn spawn_job<F, T>(&self, fut: F) -> JobHandle<T>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let inner = self.inner.clone();
        let shutdown = inner.shutdown.clone();
        r2e_core::rt::spawn_ctl(async move {
            let permit = match inner.semaphore.clone().try_acquire_owned() {
                Ok(p) => p,
                Err(tokio::sync::TryAcquireError::Closed) => {
                    panic!("executor shut down while job was pending");
                }
                Err(tokio::sync::TryAcquireError::NoPermits) => {
                    inner.queued.fetch_add(1, Ordering::Relaxed);
                    let p = tokio::select! {
                        biased;
                        _ = shutdown.cancelled() => {
                            inner.queued.fetch_sub(1, Ordering::Relaxed);
                            panic!("executor shut down while job was queued");
                        }
                        p = inner.semaphore.clone().acquire_owned() => match p {
                            Ok(p) => p,
                            Err(_) => {
                                inner.queued.fetch_sub(1, Ordering::Relaxed);
                                panic!("executor shut down while job was queued");
                            }
                        }
                    };
                    inner.queued.fetch_sub(1, Ordering::Relaxed);
                    p
                }
            };
            inner.drain_count.fetch_add(1, Ordering::Relaxed);
            let result = fut.await;
            drop(permit);
            let prev = inner.drain_count.fetch_sub(1, Ordering::Relaxed);
            inner.completed.fetch_add(1, Ordering::Relaxed);
            if prev == 1 && inner.shutdown.is_cancelled() {
                inner.notify_idle.notify_waiters();
            }
            result
        })
    }

    /// Read a snapshot of the executor's metrics.
    pub fn metrics(&self) -> ExecutorMetrics {
        let running = if self.inner.shutdown.is_cancelled() {
            self.inner.drain_count.load(Ordering::Relaxed)
        } else {
            self.inner.max_concurrent - self.inner.semaphore.available_permits() as u64
        };
        ExecutorMetrics {
            queued: self.inner.queued.load(Ordering::Relaxed),
            running,
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

    /// Configured graceful-shutdown timeout (from [`ExecutorConfig::shutdown_timeout`]).
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
                let waiter = self.inner.notify_idle.notified();
                if self.inner.drain_count.load(Ordering::Acquire) == 0 {
                    return;
                }
                waiter.await;
            }
        };
        r2e_core::rt::timeout(timeout, drain).await.is_ok()
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
    type Provided = (PoolExecutor,);
    type Deps = ();
    type LateDeps = ();
    type Config = ();

    fn install(&mut self, _deps: (), ctx: &mut PluginInstallContext<'_>) -> (PoolExecutor,) {
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

        ctx.on_shutdown_async(move || async move {
            let timeout = shutdown_handle.shutdown_timeout();
            if timeout.is_zero() {
                shutdown_handle.shutdown();
                return;
            }
            let _ = shutdown_handle.shutdown_graceful(timeout).await;
        });

        (executor,)
    }
}
