//! Async-runtime facade for r2e.
//!
//! # Purpose
//!
//! This module centralises every direct `tokio::*` touchpoint in r2e crates so
//! that a future thread-per-core migration (sharded current-thread runtimes) can
//! swap the underlying runtime in one place instead of hunting across dozens of
//! call sites.
//!
//! # What is in scope
//!
//! - Task spawning: [`spawn`], [`spawn_blocking`] → [`JobHandle<T>`]
//! - Time: [`sleep`], [`timeout`], [`interval`]
//! - Network: [`bind_tcp`], [`lookup_host`]
//! - Signals: [`shutdown_signal`]
//!
//! # Explicitly out of scope
//!
//! `tokio::sync` primitives (`broadcast`, `Notify`, `Semaphore`, `RwLock`,
//! `mpsc`, `oneshot`) are NOT wrapped here.  They are runtime-agnostic in
//! practice and wrapping them would add complexity with no benefit.
//!
//! # Known facade exceptions
//!
//! - `r2e-http/src/quic.rs` — `tokio::spawn` is called directly there because
//!   `r2e-http` sits *below* `r2e-core` in the dependency graph (r2e-core
//!   depends on r2e-http) and therefore cannot use this module.  The quinn/h3
//!   libraries are tokio-bound anyway.
//! - `r2e-core/src/sharded.rs` — constructs `current_thread` worker runtimes
//!   via `tokio::runtime::Builder` directly.  Runtime *construction* is the
//!   sharding mechanism itself, not a touchpoint to abstract.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

// Re-export Interval and MissedTickBehavior from tokio directly.
//
// Wrapping `tokio::time::Interval` is disproportionate: it has many methods
// (tick, reset, reset_at, set_missed_tick_behavior, …) and callers use
// MissedTickBehavior variants by name.  Both types are runtime-flavour-neutral
// structs.  Re-exporting them keeps migration straightforward if the runtime
// ever changes.
pub use tokio::time::{Interval, MissedTickBehavior};

// ── JoinError ─────────────────────────────────────────────────────────────────

/// The error returned when a [`JobHandle`] is awaited and the task failed.
///
/// The inner `tokio::task::JoinError` is private to keep the public API
/// decoupled from tokio.
pub struct JoinError(tokio::task::JoinError);

impl JoinError {
    /// Returns `true` if the task panicked.
    pub fn is_panic(&self) -> bool {
        self.0.is_panic()
    }

    /// Returns `true` if the task was cancelled via [`JobHandle::abort`].
    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
}

impl std::fmt::Debug for JoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::fmt::Display for JoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for JoinError {}

// ── JobHandle<T> ─────────────────────────────────────────────────────────────

/// An opaque handle to a spawned task.
///
/// Returned by [`spawn`].  Implements `Future<Output = Result<T, JoinError>>`.
///
/// The inner `tokio::task::JoinHandle<T>` is private to decouple callers from
/// tokio's type.
pub struct JobHandle<T>(tokio::task::JoinHandle<T>);

impl<T> JobHandle<T> {
    /// Abort the task.  The task will receive a cancellation signal and resolve
    /// to `Err(JoinError::is_cancelled())` when awaited.
    pub fn abort(&self) {
        self.0.abort();
    }

    /// Returns `true` if the task has finished (succeeded, panicked, or was
    /// aborted).
    pub fn is_finished(&self) -> bool {
        self.0.is_finished()
    }
}

impl<T> Future for JobHandle<T> {
    type Output = Result<T, JoinError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx).map_err(JoinError)
    }
}

// ── Timeout error ─────────────────────────────────────────────────────────────

/// Error returned by [`timeout`] when the deadline elapses.
///
/// Wraps `tokio::time::error::Elapsed` privately.
pub struct Elapsed(tokio::time::error::Elapsed);

impl std::fmt::Debug for Elapsed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::fmt::Display for Elapsed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for Elapsed {}

// ── Public surface ────────────────────────────────────────────────────────────

/// Spawn an async task on the runtime, returning a [`JobHandle<T>`].
///
/// Equivalent to `tokio::spawn`.
pub fn spawn<F, T>(future: F) -> JobHandle<T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    JobHandle(tokio::spawn(future))
}

/// Run a blocking closure on the runtime's blocking thread pool, returning a
/// [`JobHandle<T>`].
///
/// Equivalent to `tokio::task::spawn_blocking`.
pub fn spawn_blocking<F, T>(f: F) -> JobHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    JobHandle(tokio::task::spawn_blocking(f))
}

/// Wait for `duration` to elapse.
///
/// Equivalent to `tokio::time::sleep`.
pub fn sleep(duration: Duration) -> tokio::time::Sleep {
    tokio::time::sleep(duration)
}

/// Run `future`, cancelling it if `duration` elapses first.
///
/// Returns `Ok(output)` or `Err(Elapsed)`.
///
/// Equivalent to `tokio::time::timeout`.
pub async fn timeout<F, T>(duration: Duration, future: F) -> Result<T, Elapsed>
where
    F: Future<Output = T>,
{
    tokio::time::timeout(duration, future)
        .await
        .map_err(Elapsed)
}

/// Create a ticker that fires at a fixed `period`.
///
/// Equivalent to `tokio::time::interval`.  Returns `tokio::time::Interval`
/// directly (see module doc for rationale).
pub fn interval(period: Duration) -> Interval {
    tokio::time::interval(period)
}

/// Bind a TCP listener on `addr`.
///
/// The concrete listener type remains `tokio::net::TcpListener` because axum
/// requires it directly.  The binding itself goes through this facade so the
/// call site is isolated.
pub async fn bind_tcp<A: tokio::net::ToSocketAddrs>(addr: A) -> io::Result<tokio::net::TcpListener> {
    tokio::net::TcpListener::bind(addr).await
}

/// Resolve `addr` to all its [`std::net::SocketAddr`] candidates using async DNS.
///
/// Returns every resolved address, in resolver order, so callers can try each
/// candidate like `tokio::net::TcpListener::bind` does (binding only the first
/// would silently drop the multi-address fallback — e.g. `localhost` resolving
/// to `::1` then `127.0.0.1`). Errors if resolution yields no address. This
/// goes through the facade (tokio's async resolver) so we never perform
/// blocking std DNS on an async thread.
pub async fn lookup_host(addr: &str) -> io::Result<Vec<std::net::SocketAddr>> {
    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host(addr).await?.collect();
    if addrs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            format!("could not resolve address: {addr}"),
        ));
    }
    Ok(addrs)
}

/// Future that resolves on Ctrl-C or SIGTERM (Unix).
///
/// This is the centralised shutdown-signal implementation extracted from
/// `builder.rs`.  It logs the received signal before returning.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to listen for Ctrl-C");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to listen for SIGTERM")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("Shutdown signal received, starting graceful shutdown");
}
