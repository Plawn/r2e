//! Bounding the HTTP drain phase of graceful shutdown.
//!
//! `with_graceful_shutdown` waits for every in-flight connection to finish and
//! offers no deadline of its own: one client holding a request (or an SSE
//! stream) open keeps the process alive forever. [`AppBuilder::drain_timeout`]
//! puts a ceiling on that wait, and this module is the one implementation of
//! it — shared by the single-listener path
//! ([`PreparedApp::run`](crate::builder::PreparedApp::run)) and by each
//! SO_REUSEPORT worker ([`crate::runtime::sharded`]), so the two strategies
//! behave identically.
//!
//! [`AppBuilder::drain_timeout`]: crate::builder::AppBuilder::drain_timeout

use crate::rt::CancelToken;
use std::future::Future;
use std::time::Duration;

/// A future that resolves `drain_timeout` after `cancel` fires — never, if no
/// timeout is configured.
///
/// The budget deliberately starts at **cancellation**, not at `serve()`: the
/// token is cancelled exactly when the shutdown future resolves, i.e. when the
/// listener stops accepting and the drain begins. Timing from the start of
/// serving would kill a healthy server after `drain_timeout` of normal
/// operation.
async fn drain_deadline(cancel: CancelToken, drain_timeout: Option<Duration>) {
    match drain_timeout {
        Some(d) => {
            cancel.cancelled().await;
            crate::rt::sleep(d).await;
        }
        // No deadline: never resolve, so the `select!` below is decided by the
        // serve future alone (the historical, unbounded behavior).
        None => std::future::pending().await,
    }
}

/// Await `serve` (an axum `with_graceful_shutdown` future), giving the drain at
/// most `drain_timeout` once `cancel` has fired.
///
/// On overflow this **drops** `serve` rather than spawning-and-aborting it.
/// Dropping is what abandons the remaining connections, and it is the only
/// option that works on both call sites: the sharded workers run their serve
/// future inside a `current_thread` runtime's `LocalSet`, where the future is
/// not required to be `Send`, and the single-listener path would have to box
/// and spawn a `'static` future purely to be able to abort it. `select!` pins
/// on the stack and needs neither. Abandoned connections are simply closed when
/// the underlying listener/IO is dropped.
///
/// Returns the serve result, or `Ok(())` when the deadline won — a drain
/// timeout is a warning, not a serve failure, so the caller proceeds to the
/// post-drain phases (tracked-handle join, then `on_stop` hooks) exactly as it
/// would after a clean drain.
///
/// Callers pass `serve.into_future()`: axum's `WithGracefulShutdown` is only an
/// `IntoFuture`, and taking `impl IntoFuture` here would make the associated
/// future type opaque to the caller's `Send` inference.
pub(crate) async fn bounded_http_drain<F>(
    serve: F,
    cancel: CancelToken,
    drain_timeout: Option<Duration>,
) -> std::io::Result<()>
where
    F: Future<Output = std::io::Result<()>>,
{
    crate::rt::select! {
        res = serve => res,
        _ = drain_deadline(cancel, drain_timeout) => {
            tracing::warn!(
                phase = "http drain",
                drain_timeout_ms = drain_timeout.map(|d| d.as_millis() as u64),
                "drain_timeout elapsed before in-flight requests finished; \
                 abandoning the remaining connections and continuing shutdown"
            );
            Ok(())
        }
    }
}
