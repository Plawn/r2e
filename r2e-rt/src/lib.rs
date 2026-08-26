//! Async-runtime facade for r2e.
//!
//! # Purpose
//!
//! This crate centralises every direct `tokio::*` touchpoint in r2e so that
//! swapping the underlying runtime — or migrating to thread-per-core sharded
//! current-thread runtimes — is a change in one place instead of a hunt across
//! dozens of call sites.
//!
//! `r2e-rt` sits at the **bottom of the workspace dependency graph**: it depends
//! on `tokio`, `tokio-util` and `tokio-stream` and on no other r2e crate, so
//! every crate — `r2e-http` included, which sits *below* `r2e-core` — can go
//! through it. `r2e-core` re-exports it as `r2e_core::rt`, which is also what
//! `r2e::rt` resolves to.
//!
//! # What is in scope
//!
//! - Task spawning: [`spawn`], [`spawn_blocking`], [`spawn_ctl`] → [`JobHandle<T>`],
//!   [`yield_now`], [`in_runtime`]
//! - Control-plane handle: [`set_control_plane`], [`current_handle`]
//! - Time: [`sleep`], [`sleep_until`], [`timeout`], [`interval`], [`Instant`]
//! - Network: [`bind_tcp`], [`lookup_host`], [`TcpListener`], [`TcpStream`]
//! - Async I/O traits: the [`io`] module (`AsyncRead`/`AsyncWrite` + their
//!   extension traits)
//! - Signals: [`shutdown_signal`]
//! - Cancellation: [`CancelToken`], [`CancelDropGuard`]
//! - Synchronisation: the [`sync`] module (`mpsc`, `oneshot`, `broadcast`,
//!   `watch`, `Mutex`, `RwLock`, `Notify`, `Semaphore`, `OnceCell`)
//! - Async control flow: [`select!`](select), [`pin!`](pin), [`join!`](join),
//!   [`JoinSet`]
//! - Streams: the [`stream`] re-export of `tokio-stream`
//! - Runtime construction: [`RuntimeBuilder`], [`Runtime`], [`RuntimeHandle`],
//!   [`block_on`], [`block_in_place`]
//!
//! # Wrapped vs re-exported
//!
//! Two different treatments, on purpose:
//!
//! - **Wrapped** (newtype, tokio type private): everything that appears in
//!   r2e's *public API* — [`JobHandle`], [`JoinError`], [`Elapsed`],
//!   [`CancelToken`]. A downstream app must be able to consume R2E's shutdown
//!   API without adding `tokio-util` to its own `Cargo.toml`, and a runtime
//!   swap must not force every app to change.
//! - **Re-exported** (identity stays tokio's): the [`sync`] primitives, the
//!   [`select!`](select)/[`pin!`](pin)/[`join!`](join) macros, [`Instant`],
//!   [`Interval`], [`MissedTickBehavior`], [`JoinSet`], [`stream`]. Their *shape* is
//!   runtime-neutral; re-exporting removes the `tokio::` name from dozens of
//!   files at zero cost, and is what makes the boundary check mechanical.
//!
//! # Control-plane / data-plane split
//!
//! In sharded mode (`server.workers`), HTTP requests are served on N
//! `current_thread` worker runtimes (the *data plane*), while all non-HTTP work
//! — scheduler tasks, services, event consumers, QUIC, executor jobs, lazy-bean
//! resolution — must run on the caller's main multi-thread runtime (the
//! *control plane*). [`spawn_ctl`] routes a future onto the control plane when a
//! worker thread has registered the control-plane handle (see
//! [`set_control_plane`]); otherwise it is byte-for-byte equivalent to
//! [`spawn`].
//!
//! # Known facade exceptions
//!
//! Documented in `plans/runtime-http-dependency-containment.md` §4 as
//! permanently allowlisted, by design:
//!
//! - **this crate** — it *is* the facade.
//! - `r2e-test`, `r2e-devservices` — test harnesses; they legitimately own a
//!   runtime. `scripts/check-source-boundary.sh` excludes them from the tokio
//!   group only, so `r2e-test` still counts for the axum one.
//!
//! `r2e-core/src/runtime/sharded.rs` used to be listed here — building the
//! per-worker `current_thread` runtimes *is* the sharding mechanism. It no
//! longer is: [`RuntimeBuilder`], [`RuntimeHandle`] and [`TcpListener`] express
//! everything it needs, so the sharded path goes through the facade like
//! everything else.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

// Re-export Instant, Interval and MissedTickBehavior from tokio directly.
//
// Wrapping `tokio::time::Interval` is disproportionate: it has many methods
// (tick, reset, reset_at, set_missed_tick_behavior, …) and callers use
// MissedTickBehavior variants by name.  `Instant` is a monotonic timestamp on
// the runtime's own clock — the currency of any deadline-driven timer wheel
// (the scheduler's min-heap), and the only clock `sleep_until` accepts.  All
// three are runtime-flavour-neutral; re-exporting them keeps migration
// straightforward if the runtime ever changes.
pub use tokio::time::{Instant, Interval, MissedTickBehavior};

/// The async TCP listener, re-exported.
///
/// Not wrapped: axum's `serve` takes this concrete type, so a newtype would be
/// unwrapped at every call site and buy nothing. Naming it `rt::TcpListener`
/// still keeps `tokio::net` out of the rest of the workspace — a runtime swap
/// rewrites this line and the two constructors below ([`bind_tcp`],
/// `TcpListener::from_std`).
pub use tokio::net::TcpListener;

/// The async TCP client socket, re-exported.
///
/// Same reasoning as [`TcpListener`]: it is the concrete type every async TCP
/// API in the ecosystem speaks, so a newtype would be unwrapped at every call
/// site. Naming it `rt::TcpStream` keeps `tokio::net` out of the rest of the
/// workspace and out of examples/tests.
pub use tokio::net::TcpStream;

/// The async UDP socket, re-exported.
///
/// Same reasoning as [`TcpStream`]. Per-worker services (see
/// `r2e_core::runtime::worker`) adopt a pre-configured `std::net::UdpSocket`
/// (SO_REUSEPORT, buffer sizes, cBPF, …) with `UdpSocket::from_std`, which must
/// run inside the owning worker's runtime.
pub use tokio::net::UdpSocket;

/// A set of `!Send` tasks pinned to the thread that runs it, re-exported.
///
/// This is the worker-local executor behind per-worker services: a sharded
/// worker drives `LocalSet::run_until(..)` on its `current_thread` runtime, and
/// [`spawn_local`] places tasks on it. Not wrapped: the only operations used
/// (`run_until`, `spawn_local`) are already exposed by the free functions here.
pub use tokio::task::LocalSet;

/// Async byte-stream traits and their extension methods.
///
/// Plain re-exports of `tokio::io`. The traits themselves are the de-facto
/// async I/O contract (`hyper`, `tonic`, `tokio-rustls` and friends are all
/// written against them), so wrapping them would buy a runtime swap nothing —
/// a swap rewrites this module's `pub use` lines, exactly like [`sync`].
pub mod io {
    pub use tokio::io::{
        AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader, BufWriter,
        duplex,
    };
}

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

    /// Consumes the error, returning the panic payload of the task.
    ///
    /// Panics if the task did not panic — check [`JoinError::is_panic`] first.
    pub fn into_panic(self) -> Box<dyn std::any::Any + Send + 'static> {
        self.0.into_panic()
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
#[expect(
    clippy::disallowed_types,
    reason = "this IS the sanctioned wrapper the workspace-wide deny points to"
)]
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
#[expect(
    clippy::disallowed_methods,
    reason = "this IS the sanctioned wrapper the workspace-wide deny points to"
)]
pub fn spawn<F, T>(future: F) -> JobHandle<T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    JobHandle(tokio::spawn(future))
}

/// Spawn a `!Send` task on the current thread's [`LocalSet`], returning a
/// [`JobHandle<T>`].
///
/// Equivalent to `tokio::task::spawn_local`. The task is pinned to the calling
/// thread and never migrates — it may own `Rc`/`RefCell` state. This is the
/// worker-local spawn used by per-worker services (`WorkerContext::spawn_local`
/// in r2e-core); it is **not** a general-purpose spawn: outside a `LocalSet`
/// context it panics, and it must never be reached from a request handler
/// (handlers run on plain runtimes, not inside a `LocalSet`).
///
/// # Panics
///
/// If called outside a [`LocalSet`] context.
#[expect(
    clippy::disallowed_methods,
    reason = "this IS the sanctioned wrapper the workspace-wide deny points to"
)]
pub fn spawn_local<F>(future: F) -> JobHandle<F::Output>
where
    F: Future + 'static,
    F::Output: 'static,
{
    JobHandle(tokio::task::spawn_local(future))
}

// ── Control plane ───────────────────────────────────────────────────────────

thread_local! {
    /// The control-plane runtime handle for this thread, if any.
    ///
    /// This is a thread-local (not a global `OnceLock`) on purpose: a process
    /// can run several r2e apps with distinct runtimes (the test suite does this
    /// constantly). A global handle would pin the first app's runtime and make
    /// later apps spawn onto a possibly-dropped runtime. The thread-local is set
    /// only on sharded worker threads (see [`set_control_plane`]), scoping the
    /// control plane to the app that created the worker.
    static CONTROL_PLANE: std::cell::RefCell<Option<RuntimeHandle>> =
        const { std::cell::RefCell::new(None) };
}

/// Register `handle` as the control-plane runtime for the current thread.
///
/// Called by each sharded worker thread at startup so that work initiated from
/// within a request handler (anything reaching [`spawn_ctl`], including
/// lazy-bean first-touch) is routed onto the caller's main multi-thread runtime
/// rather than the worker's `current_thread` runtime.
pub fn set_control_plane(handle: RuntimeHandle) {
    CONTROL_PLANE.with(|cp| *cp.borrow_mut() = Some(handle));
}

/// Returns the control-plane handle registered for the current thread, if any.
///
/// Callers outside the facade should prefer [`spawn_ctl`]; this is exposed for
/// the one place that needs to *block* on the control plane rather than spawn
/// onto it (lazy-bean first-touch resolution).
pub fn control_plane_handle() -> Option<RuntimeHandle> {
    CONTROL_PLANE.with(|cp| cp.borrow().clone())
}

/// The handle of the runtime currently driving this thread.
///
/// Shorthand for [`RuntimeHandle::current`]. Panics if called outside a runtime
/// context.
#[must_use]
pub fn current_handle() -> RuntimeHandle {
    RuntimeHandle::current()
}

/// Spawn a task on the control-plane runtime, returning a [`JobHandle<T>`].
///
/// When a control-plane handle has been registered on the current thread (only
/// on sharded worker threads, via [`set_control_plane`]), the future is spawned
/// onto that runtime — keeping background work off the HTTP worker runtimes.
/// When no handle is registered (the default, non-sharded path), this is
/// byte-for-byte equivalent to [`spawn`].
#[expect(
    clippy::disallowed_methods,
    reason = "this IS the sanctioned wrapper the workspace-wide deny points to"
)]
pub fn spawn_ctl<F, T>(future: F) -> JobHandle<T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    match control_plane_handle() {
        Some(handle) => handle.spawn(future),
        None => JobHandle(tokio::spawn(future)),
    }
}

/// Run a blocking closure on the runtime's blocking thread pool, returning a
/// [`JobHandle<T>`].
///
/// Equivalent to `tokio::task::spawn_blocking`.
#[expect(
    clippy::disallowed_methods,
    reason = "this IS the sanctioned wrapper the workspace-wide deny points to"
)]
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

/// Wait until `deadline`.
///
/// The deadline form of [`sleep`], on the runtime's own [`Instant`] clock —
/// what a timer wheel driven by absolute fire times needs (the scheduler's
/// min-heap driver).
pub fn sleep_until(deadline: Instant) -> tokio::time::Sleep {
    tokio::time::sleep_until(deadline)
}

/// Yield back to the scheduler, letting other ready tasks run.
///
/// Equivalent to `tokio::task::yield_now`.
pub async fn yield_now() {
    tokio::task::yield_now().await;
}

/// Run a blocking section inside an async task without starving the runtime.
///
/// Tells the runtime this thread is about to block so it can move the other
/// tasks it was driving elsewhere. Only valid on a multi-thread runtime — it
/// **panics** on a `current_thread` one, so gate it on
/// [`RuntimeHandle::is_multi_thread`].
///
/// Equivalent to `tokio::task::block_in_place`.
pub fn block_in_place<F, T>(f: F) -> T
where
    F: FnOnce() -> T,
{
    tokio::task::block_in_place(f)
}

/// Whether the calling thread is currently driven by a runtime.
///
/// The non-panicking probe behind [`current_handle`]: synchronous paths that
/// may run outside a runtime (a `Drop` impl detaching cleanup work) check this
/// before spawning.
#[must_use]
#[expect(
    clippy::disallowed_types,
    reason = "this IS the sanctioned wrapper the workspace-wide deny points to"
)]
pub fn in_runtime() -> bool {
    tokio::runtime::Handle::try_current().is_ok()
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
/// The concrete listener type remains [`TcpListener`] because axum requires it
/// directly.  The binding itself goes through this facade so the call site is
/// isolated.
pub async fn bind_tcp<A: tokio::net::ToSocketAddrs>(addr: A) -> std::io::Result<TcpListener> {
    TcpListener::bind(addr).await
}

/// Resolve `addr` to all its [`std::net::SocketAddr`] candidates using async DNS.
///
/// Returns every resolved address, in resolver order, so callers can try each
/// candidate like `tokio::net::TcpListener::bind` does (binding only the first
/// would silently drop the multi-address fallback — e.g. `localhost` resolving
/// to `::1` then `127.0.0.1`). Errors if resolution yields no address. This
/// goes through the facade (tokio's async resolver) so we never perform
/// blocking std DNS on an async thread.
pub async fn lookup_host(addr: &str) -> std::io::Result<Vec<std::net::SocketAddr>> {
    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host(addr).await?.collect();
    if addrs.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AddrNotAvailable,
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

// ── Async control-flow macros ────────────────────────────────────────────────

// Re-exported, not wrapped: these are macros over `Future`, so they keep
// working over r2e futures whatever drives them.  Re-exporting removes the
// `tokio::` name from ~20 files at zero cost.
pub use tokio::{join, pin, select};

// ── Task grouping ────────────────────────────────────────────────────────────

/// A collection of spawned tasks awaited as a group.
///
/// Re-exported from tokio: `JoinSet` is a container, not a runtime flavour, and
/// callers use its full method surface (`spawn`, `join_next`, `abort_all`,
/// `shutdown`, `len`).
pub use tokio::task::JoinSet;

// ── Streams ──────────────────────────────────────────────────────────────────

/// The `tokio-stream` crate, re-exported so call sites name `r2e_rt::stream`
/// instead of `tokio_stream` (`StreamExt`, `wrappers::BroadcastStream`, …).
pub use tokio_stream as stream;

// ── Synchronisation primitives ───────────────────────────────────────────────

/// Async synchronisation primitives.
///
/// Plain re-exports of `tokio::sync`: their shape is runtime-neutral, but their
/// *identity* is tokio's, and re-exporting is what removes the `tokio::sync`
/// name from the workspace at zero cost. Wrapping them would buy nothing a
/// runtime swap could use — a swap rewrites this module's `pub use` lines.
pub mod sync {
    pub use tokio::sync::{broadcast, mpsc, oneshot, watch};
    pub use tokio::sync::{
        Mutex, MutexGuard, Notify, OnceCell, OwnedSemaphorePermit, RwLock, RwLockReadGuard,
        RwLockWriteGuard, Semaphore, TryAcquireError,
    };
}

// ── Cancellation ─────────────────────────────────────────────────────────────

/// A cancellation signal shared by a tree of tasks.
///
/// Wraps `tokio_util::sync::CancellationToken` as a **newtype** rather than
/// re-exporting it, because this type is in r2e's public API
/// (`ServeContext::shutdown_token`, `ConfigWatchContext`, `LiveConfigStream::drive`).
/// A downstream app consuming R2E's shutdown API must not have to add
/// `tokio-util` to its own `Cargo.toml`, and a runtime swap must not force
/// every app to change.
///
/// Cancellation is level-triggered and permanent: once [`cancel`](Self::cancel)
/// has been called, [`is_cancelled`](Self::is_cancelled) stays `true` and
/// [`cancelled`](Self::cancelled) resolves immediately, forever. Clones share
/// one signal; [`child_token`](Self::child_token) creates a token that can be
/// cancelled on its own **and** is cancelled with its parent.
#[expect(
    clippy::disallowed_types,
    reason = "this IS the sanctioned wrapper the workspace-wide deny points to"
)]
#[derive(Clone, Debug, Default)]
pub struct CancelToken(tokio_util::sync::CancellationToken);

impl CancelToken {
    /// Create a fresh, un-cancelled token.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a token cancelled by [`cancel`](Self::cancel) on itself **or** on
    /// this (parent) token.
    #[must_use]
    pub fn child_token(&self) -> Self {
        Self(self.0.child_token())
    }

    /// Cancel this token and every child of it. Idempotent.
    pub fn cancel(&self) {
        self.0.cancel();
    }

    /// Whether [`cancel`](Self::cancel) has already fired.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }

    /// Resolves once the token is cancelled — immediately if it already is.
    ///
    /// Cancellation-safe: usable as a [`select!`](select) branch.
    pub async fn cancelled(&self) {
        self.0.cancelled().await;
    }

    /// The owned form of [`cancelled`](Self::cancelled): a `'static` future,
    /// for the callers that must hand cancellation to something outliving the
    /// borrow (axum's `with_graceful_shutdown`, a spawned task).
    #[must_use]
    pub fn cancelled_owned(self) -> impl Future<Output = ()> + Send + 'static {
        self.0.cancelled_owned()
    }

    /// Convert into a guard that cancels the token when dropped.
    ///
    /// The way to make cancellation survive the *uncontrolled* exits — a panic
    /// unwinding out of the owner, or the whole future being dropped.
    #[must_use]
    pub fn drop_guard(self) -> CancelDropGuard {
        CancelDropGuard(self.0.drop_guard())
    }

    /// The wrapped tokio-util token.
    ///
    /// Migration seam: crates not yet moved onto [`CancelToken`] take a raw
    /// `CancellationToken`, so the flipped r2e-core APIs hand them one through
    /// `.into()` / this accessor. Not part of the stable surface.
    #[doc(hidden)]
    #[must_use]
    #[expect(
        clippy::disallowed_types,
        reason = "this IS the sanctioned wrapper the workspace-wide deny points to"
    )]
    pub fn into_inner(self) -> tokio_util::sync::CancellationToken {
        self.0
    }

    /// Borrow the wrapped tokio-util token. See [`into_inner`](Self::into_inner).
    #[doc(hidden)]
    #[must_use]
    #[expect(
        clippy::disallowed_types,
        reason = "this IS the sanctioned wrapper the workspace-wide deny points to"
    )]
    pub fn inner(&self) -> &tokio_util::sync::CancellationToken {
        &self.0
    }
}

#[expect(
    clippy::disallowed_types,
    reason = "this IS the sanctioned wrapper the workspace-wide deny points to"
)]
impl From<tokio_util::sync::CancellationToken> for CancelToken {
    fn from(token: tokio_util::sync::CancellationToken) -> Self {
        Self(token)
    }
}

#[expect(
    clippy::disallowed_types,
    reason = "this IS the sanctioned wrapper the workspace-wide deny points to"
)]
impl From<CancelToken> for tokio_util::sync::CancellationToken {
    fn from(token: CancelToken) -> Self {
        token.0
    }
}

/// Guard returned by [`CancelToken::drop_guard`] — cancels the token on drop.
///
/// [`disarm`](Self::disarm) gives the token back without cancelling it.
#[derive(Debug)]
pub struct CancelDropGuard(tokio_util::sync::DropGuard);

impl CancelDropGuard {
    /// Give up the guard without cancelling, returning the token.
    #[must_use]
    pub fn disarm(self) -> CancelToken {
        CancelToken(self.0.disarm())
    }
}

// ── Runtime construction ─────────────────────────────────────────────────────

/// A driving runtime, built by [`RuntimeBuilder`].
///
/// Wraps `tokio::runtime::Runtime` so `#[r2e::main]` / `#[r2e::test]` can emit
/// `::r2e_rt::` paths instead of `::tokio::runtime::` ones.
#[derive(Debug)]
pub struct Runtime(tokio::runtime::Runtime);

impl Runtime {
    /// Run `future` to completion on this runtime, blocking the current thread.
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.0.block_on(future)
    }

    /// A handle to this runtime, for spawning onto it from elsewhere.
    #[must_use]
    pub fn handle(&self) -> RuntimeHandle {
        RuntimeHandle(self.0.handle().clone())
    }
}

/// A cloneable handle to a runtime, used to reach it from another thread.
///
/// Wraps `tokio::runtime::Handle` so the sharded worker plumbing and the
/// lazy-bean off-runtime resolution path never name `tokio::runtime`.
#[expect(
    clippy::disallowed_types,
    reason = "this IS the sanctioned wrapper the workspace-wide deny points to"
)]
#[derive(Clone, Debug)]
pub struct RuntimeHandle(tokio::runtime::Handle);

impl RuntimeHandle {
    /// The handle of the runtime driving the current thread.
    ///
    /// # Panics
    ///
    /// If the calling thread is not driven by a runtime — use
    /// [`try_current`](Self::try_current) (or [`in_runtime`]) when that is
    /// possible.
    #[must_use]
    #[expect(
        clippy::disallowed_types,
        reason = "this IS the sanctioned wrapper the workspace-wide deny points to"
    )]
    pub fn current() -> Self {
        Self(tokio::runtime::Handle::current())
    }

    /// The handle of the runtime driving the current thread, or `None`.
    #[must_use]
    #[expect(
        clippy::disallowed_types,
        reason = "this IS the sanctioned wrapper the workspace-wide deny points to"
    )]
    pub fn try_current() -> Option<Self> {
        tokio::runtime::Handle::try_current().ok().map(Self)
    }

    /// Spawn a task onto *this* runtime, wherever the caller runs.
    ///
    /// Unlike [`spawn`], which uses the current thread's runtime.
    #[expect(
        clippy::disallowed_methods,
        reason = "this IS the sanctioned wrapper the workspace-wide deny points to"
    )]
    pub fn spawn<F, T>(&self, future: F) -> JobHandle<T>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        JobHandle(self.0.spawn(future))
    }

    /// Drive `future` to completion on this runtime, blocking the calling
    /// thread.
    ///
    /// # Panics
    ///
    /// If called from a thread already being driven by a runtime — wrap it in
    /// [`block_in_place`] when that is the case.
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.0.block_on(future)
    }

    /// Whether this runtime is the work-stealing multi-thread flavour.
    ///
    /// The two things that need to know: [`block_in_place`] (panics on
    /// `current_thread`) and the sharded control-plane check.
    #[must_use]
    pub fn is_multi_thread(&self) -> bool {
        self.0.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
    }
}

/// Builder for a [`Runtime`].
///
/// Deliberately minimal: exactly the knobs `#[r2e::main]`, `#[r2e::test]` and
/// `#[r2e::test_suite]` accept (see `r2e-macros/src/util/runtime_args.rs`), so
/// the macros never have to name `tokio::runtime::Builder`.
#[derive(Debug)]
pub struct RuntimeBuilder(tokio::runtime::Builder);

impl RuntimeBuilder {
    /// A runtime driving all tasks on the calling thread.
    #[must_use]
    pub fn new_current_thread() -> Self {
        Self(tokio::runtime::Builder::new_current_thread())
    }

    /// A work-stealing runtime over a pool of worker threads.
    #[must_use]
    pub fn new_multi_thread() -> Self {
        Self(tokio::runtime::Builder::new_multi_thread())
    }

    /// Number of worker threads (multi-thread flavour only).
    #[must_use]
    pub fn worker_threads(mut self, n: usize) -> Self {
        self.0.worker_threads(n);
        self
    }

    /// Upper bound on threads spawned for blocking work.
    #[must_use]
    pub fn max_blocking_threads(mut self, n: usize) -> Self {
        self.0.max_blocking_threads(n);
        self
    }

    /// Stack size, in bytes, of each spawned thread.
    #[must_use]
    pub fn thread_stack_size(mut self, n: usize) -> Self {
        self.0.thread_stack_size(n);
        self
    }

    /// Name given to each spawned thread.
    #[must_use]
    pub fn thread_name(mut self, name: impl Into<String>) -> Self {
        self.0.thread_name(name.into());
        self
    }

    /// Ticks between polls of the global task queue.
    #[must_use]
    pub fn global_queue_interval(mut self, n: u32) -> Self {
        self.0.global_queue_interval(n);
        self
    }

    /// Ticks between polls of the I/O and timer drivers.
    #[must_use]
    pub fn event_interval(mut self, n: u32) -> Self {
        self.0.event_interval(n);
        self
    }

    /// How long an idle blocking thread is kept alive.
    #[must_use]
    pub fn thread_keep_alive(mut self, duration: Duration) -> Self {
        self.0.thread_keep_alive(duration);
        self
    }

    /// Start the clock paused, auto-advancing when every task is idle.
    ///
    /// Requires the `test-util` feature (it maps onto `tokio/test-util`, which
    /// changes timer behaviour and is therefore not on by default).
    #[cfg(feature = "test-util")]
    #[must_use]
    pub fn start_paused(mut self, paused: bool) -> Self {
        self.0.start_paused(paused);
        self
    }

    /// Enable the I/O, time and signal drivers.
    #[must_use]
    pub fn enable_all(mut self) -> Self {
        self.0.enable_all();
        self
    }

    /// Build the runtime.
    pub fn build(mut self) -> std::io::Result<Runtime> {
        self.0.build().map(Runtime)
    }
}

/// Run `future` to completion on a fresh single-threaded runtime, blocking the
/// calling thread.
///
/// For the synchronous entry points that need to reach async code once (CLI
/// commands, build helpers, `main` shims). Inside an async context, `.await`
/// instead — calling this from a runtime thread panics, exactly as tokio's own
/// `Runtime::block_on` does.
///
/// # Panics
///
/// If the runtime cannot be built, or if called from within a runtime.
pub fn block_on<F: Future>(future: F) -> F::Output {
    RuntimeBuilder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build r2e runtime")
        .block_on(future)
}
