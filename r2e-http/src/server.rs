//! The one serve path: a bound listener + a [`Router`] → a running HTTP server.
//!
//! Every serve path in the workspace goes through [`serve`]: the
//! single-listener `run()`, each sharded worker, and the in-process
//! `TestServer`. It is the only place that speaks the backend's serve API
//! (`axum::serve`, `ListenerExt::tap_io`, `into_make_service_with_connect_info`),
//! so a backend swap rewrites this module and nothing above it
//! (`plans/axum-containment-roadmap.md`, phase A). The source boundary check
//! (`scripts/check-source-boundary.sh`, group `axum-serve`) keeps it that way.
//!
//! What it fixes for every caller:
//!
//! - `ConnectInfo<SocketAddr>` is always installed, so peer-address guards
//!   (`PeerAddr`, per-IP rate limiting) see a client address on every path,
//!   production or test.
//! - `TCP_NODELAY` is applied to each accepted connection when
//!   [`ServeOptions::tcp_nodelay`] is set (`server.tcp_nodelay`, default `true`
//!   in `AppBuilder`). A failure to set it is logged and never fatal.
//! - Graceful shutdown: the returned future stops accepting once `shutdown`
//!   resolves and completes when the in-flight connections have finished.
//!   Bounding that drain (`drain_timeout`) is the caller's job — `r2e-core`
//!   races this future against the deadline in `bounded_http_drain` — so this
//!   module needs no knowledge of cancel tokens or budgets.
//!
//! The listener is the runtime facade's [`r2e_rt::TcpListener`]: the backend
//! requires that concrete type, which is why `r2e-rt` sits below `r2e-http` in
//! the dependency graph.

use std::future::{Future, IntoFuture as _};
use std::io;
use std::net::SocketAddr;

use crate::Router;

/// Per-server knobs for [`serve`].
///
/// Built with [`ServeOptions::default`] (nothing enabled) and the chaining
/// setters, so adding a knob later is not a breaking change for callers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ServeOptions {
    tcp_nodelay: bool,
}

impl ServeOptions {
    /// No option set: no `TCP_NODELAY`. Same as [`Default`].
    pub const fn new() -> Self {
        Self { tcp_nodelay: false }
    }

    /// Set `TCP_NODELAY` on every accepted connection (disables Nagle's
    /// algorithm, which otherwise delays small writes such as chunked bodies
    /// and SSE events). `AppBuilder` reads this from `server.tcp_nodelay`.
    pub const fn tcp_nodelay(mut self, enabled: bool) -> Self {
        self.tcp_nodelay = enabled;
        self
    }

    /// Whether `TCP_NODELAY` is applied to accepted connections.
    pub const fn is_tcp_nodelay(&self) -> bool {
        self.tcp_nodelay
    }
}

/// Serve `router` on `listener` until `shutdown` resolves, then drain.
///
/// Returns the server future; nothing runs until it is polled. The future
/// resolves `Ok(())` once every connection accepted before `shutdown` fired has
/// finished, or `Err` if accepting failed. `ConnectInfo<SocketAddr>` is
/// installed unconditionally (see the module docs).
///
/// # Example
///
/// ```rust,no_run
/// # async fn demo(router: r2e_http::Router) -> std::io::Result<()> {
/// use r2e_http::server::{serve, ServeOptions};
///
/// let listener = r2e_rt::bind_tcp("127.0.0.1:3000").await?;
/// let shutdown = r2e_rt::shutdown_signal();
/// serve(listener, router, shutdown, ServeOptions::default().tcp_nodelay(true)).await
/// # }
/// ```
pub fn serve<S>(
    listener: r2e_rt::TcpListener,
    router: Router,
    shutdown: S,
    options: ServeOptions,
) -> impl Future<Output = io::Result<()>> + Send + 'static
where
    S: Future<Output = ()> + Send + 'static,
{
    use axum::serve::ListenerExt as _;

    let nodelay = options.tcp_nodelay;
    // One tap for both settings of the flag: the branch costs a predictable
    // compare per accept, and it keeps the returned future a single type
    // without boxing.
    let listener = listener.tap_io(move |stream| {
        if nodelay && let Err(e) = stream.set_nodelay(true) {
            tracing::warn!(error = %e, "failed to set TCP_NODELAY on accepted connection");
        }
    });
    let svc = router.into_make_service_with_connect_info::<SocketAddr>();
    axum::serve(listener, svc)
        .with_graceful_shutdown(shutdown)
        .into_future()
}
