//! SO_REUSEPORT sharded serving — option A of the thread-per-core plan.
//!
//! When `server.workers` is configured, R2E serves HTTP with `N` worker
//! threads, each running its own `current_thread` Tokio runtime with its own
//! `SO_REUSEPORT` listener bound to the same address. The kernel distributes
//! incoming connections across the per-worker listeners, so there is no
//! work-stealing on the accept path. Axum (and the whole ecosystem) is kept
//! unchanged — each worker simply serves a clone of the same router.
//!
//! This module owns the socket plumbing and the worker-thread orchestration.
//! The surrounding lifecycle (consumers, startup/serve hooks, shutdown phase,
//! QUIC) lives in [`crate::builder`] and is shared with the single-listener
//! path.
//!
//! # Platform support
//!
//! `SO_REUSEPORT` (via `socket2::Socket::set_reuse_port`) is only available on
//! unix targets, excluding solaris/illumos/cygwin. The sharded serving entry
//! point is gated to those platforms; on unsupported platforms configuring
//! `server.workers` returns a clear error (see
//! [`crate::builder::PreparedApp::run`]).
//!
//! # Hot-reload
//!
//! Sharding + hot-reload (`dev-reload`) is explicitly unsupported in v1. When
//! both are requested, sharding is ignored and the single-listener path is
//! used (with a `tracing::warn!`).
//!
//! # Control plane / data plane
//!
//! Each worker runs a `current_thread` runtime and serves HTTP requests only
//! (the *data plane*). All non-HTTP work — scheduler tasks, services, event
//! consumers, QUIC, executor jobs — runs on the caller's main multi-thread
//! runtime (the *control plane*), which keeps driving the lifecycle while the
//! workers serve. Each worker thread registers the control-plane handle via
//! [`crate::rt::set_control_plane`] before entering its runtime, so background
//! work initiated from within a request handler (anything reaching
//! [`crate::rt::spawn_ctl`]) is routed back onto the control plane rather than
//! the worker's `current_thread` runtime.
//!
//! # Lazy beans
//!
//! A lazy bean first touched from within a worker is resolved on the
//! control-plane runtime: because the worker registered the control-plane
//! handle, [`crate::lazy`]'s `resolve_lazy_factory` spawns the factory on the
//! control plane and blocks the worker on a channel for the result (it cannot
//! use `block_in_place`, which panics on current-thread runtimes). No hidden
//! `lazy-fallback-runtime` is spun up. In practice lazy beans are resolved once
//! during state construction on the main runtime, so the worker path only bites
//! if a lazy bean is first touched from a worker.

use crate::config::R2eConfig;

/// Upper bound for `server.workers`. Generously above any real core count;
/// values beyond it are almost certainly config typos.
pub const MAX_WORKERS: i64 = 1024;

/// Parse the `server.workers` configuration value.
///
/// Accepted forms:
/// - absent → `Ok(None)` (single-listener behavior, unchanged default)
/// - a positive integer `n >= 1` → `Ok(Some(n))`
/// - the string `"per-core"` → `Ok(Some(available_parallelism))`
///
/// Anything else (0, negative, other strings) is a hard error — never a
/// silent fallback.
pub fn parse_workers(config: Option<&R2eConfig>) -> Result<Option<usize>, String> {
    let Some(config) = config else {
        return Ok(None);
    };
    if !config.contains_key("server.workers") {
        return Ok(None);
    }

    // Try integer first.
    if let Some(n) = config.try_get::<i64>("server.workers") {
        if n < 1 {
            return Err(format!(
                "server.workers must be a positive integer or \"per-core\", got {n}"
            ));
        }
        // Sanity cap: a typo like an extra digit should be a clear config
        // error, not FD/thread exhaustion at bind time.
        if n > MAX_WORKERS {
            return Err(format!(
                "server.workers must be at most {MAX_WORKERS}, got {n}"
            ));
        }
        return Ok(Some(n as usize));
    }

    // Fall back to the string form.
    if let Some(s) = config.try_get::<String>("server.workers") {
        if s == "per-core" {
            let n = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1);
            return Ok(Some(n));
        }
        return Err(format!(
            "server.workers must be a positive integer or \"per-core\", got \"{s}\""
        ));
    }

    Err("server.workers must be a positive integer or \"per-core\"".to_string())
}

/// Error message returned when `server.workers` is set on a platform that does
/// not support `SO_REUSEPORT`.
pub const UNSUPPORTED_PLATFORM_MSG: &str =
    "server.workers (SO_REUSEPORT sharding) is not supported on this platform";

#[cfg(all(
    unix,
    not(any(target_os = "solaris", target_os = "illumos", target_os = "cygwin"))
))]
mod imp {
    use std::net::SocketAddr;
    use tokio_util::sync::CancellationToken;

    /// Create a `SO_REUSEPORT` listener bound to `addr`, returned as a
    /// non-blocking `std::net::TcpListener` ready for
    /// `tokio::net::TcpListener::from_std`.
    ///
    /// `set_nonblocking(true)` is MANDATORY — `from_std` requires it.
    fn make_reuseport_listener(addr: SocketAddr) -> std::io::Result<std::net::TcpListener> {
        use socket2::{Domain, Protocol, Socket, Type};

        let domain = if addr.is_ipv4() {
            Domain::IPV4
        } else {
            Domain::IPV6
        };
        let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;
        socket.set_reuse_address(true)?;
        socket.set_reuse_port(true)?;
        socket.bind(&addr.into())?;
        socket.listen(1024)?;
        let std_listener: std::net::TcpListener = socket.into();
        // MANDATORY for tokio::net::TcpListener::from_std.
        std_listener.set_nonblocking(true)?;
        Ok(std_listener)
    }

    /// Serve `router` across `workers` worker threads, each with its own
    /// `current_thread` runtime and `SO_REUSEPORT` listener.
    ///
    /// `addrs` holds the resolved bind-address candidates, in resolver order.
    /// The first listener tries each candidate until one binds (mirroring
    /// `tokio::net::TcpListener::bind`'s multi-address fallback); the
    /// remaining workers then bind that listener's concrete `local_addr()`.
    /// Going through `local_addr()` also makes port `0` work: the kernel
    /// assigns the ephemeral port once, and every worker shares it.
    ///
    /// Blocks until `cancel_token` is cancelled (each worker observes a child
    /// token via graceful shutdown), then joins all worker threads.
    ///
    /// Returns the first worker serve error, if any. Worker panics are logged
    /// via `tracing::error!`.
    pub fn serve_sharded(
        router: crate::http::Router,
        addrs: &[SocketAddr],
        workers: usize,
        tcp_nodelay: bool,
        control_plane: tokio::runtime::Handle,
        cancel_token: CancellationToken,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Pre-create the listeners on the main thread so that a bind failure
        // surfaces synchronously as a run error (rather than from inside a
        // worker thread). Each worker gets its own SO_REUSEPORT socket.
        let mut last_err: Option<std::io::Error> = None;
        let mut first_listener = None;
        for candidate in addrs {
            match make_reuseport_listener(*candidate) {
                Ok(l) => {
                    first_listener = Some(l);
                    break;
                }
                Err(e) => {
                    tracing::debug!(addr = %candidate, error = %e, "sharded bind candidate failed");
                    last_err = Some(e);
                }
            }
        }
        let Some(first_listener) = first_listener else {
            // Mirror tokio's bind: surface the last bind error.
            return Err(match last_err {
                Some(e) => Box::new(e),
                None => format!("no addresses to bind for sharded serving: {addrs:?}").into(),
            });
        };
        // Concrete address the remaining workers must share. Resolves port 0
        // to the kernel-assigned ephemeral port.
        let addr = first_listener.local_addr()?;

        let mut std_listeners = Vec::with_capacity(workers);
        std_listeners.push(first_listener);
        for _ in 1..workers {
            std_listeners.push(make_reuseport_listener(addr)?);
        }

        let mut handles = Vec::with_capacity(workers);
        for (i, std_listener) in std_listeners.into_iter().enumerate() {
            let router = router.clone();
            let child_token = cancel_token.child_token();
            let control_plane = control_plane.clone();
            let handle = std::thread::Builder::new()
                .name(format!("r2e-worker-{i}"))
                .spawn(move || -> Result<(), String> {
                    // Register the control-plane handle so background work
                    // initiated from request handlers (rt::spawn_ctl) and
                    // lazy-bean first-touch run on the main multi-thread
                    // runtime, not this worker's current_thread runtime.
                    crate::rt::set_control_plane(control_plane);
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| format!("failed to build worker runtime: {e}"))?;
                    rt.block_on(async move {
                        // `from_std` must run inside the worker's runtime
                        // context.
                        let listener = tokio::net::TcpListener::from_std(std_listener)
                            .map_err(|e| format!("failed to adopt worker listener: {e}"))?;
                        let svc = router.into_make_service_with_connect_info::<SocketAddr>();
                        let shutdown = child_token.cancelled_owned();
                        let serve_result = if tcp_nodelay {
                            use crate::http::ListenerExt as _;
                            crate::http::serve(
                                listener.tap_io(|stream| {
                                    if let Err(e) = stream.set_nodelay(true) {
                                        tracing::warn!(
                                            error = %e,
                                            "failed to set TCP_NODELAY on accepted connection"
                                        );
                                    }
                                }),
                                svc,
                            )
                            .with_graceful_shutdown(shutdown)
                            .await
                        } else {
                            crate::http::serve(listener, svc)
                                .with_graceful_shutdown(shutdown)
                                .await
                        };
                        serve_result.map_err(|e| format!("worker serve error: {e}"))
                    })
                })
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    format!("failed to spawn worker thread {i}: {e}").into()
                })?;
            handles.push((i, handle));
        }

        tracing::info!(%addr, workers, "R2E server listening (sharded, SO_REUSEPORT)");

        // Block the main thread until shutdown is signalled, then join the
        // workers. We are already past the point where the main runtime drives
        // the shutdown future, so a blocking join here is acceptable.
        let mut first_err: Option<Box<dyn std::error::Error + Send + Sync>> = None;
        for (i, handle) in handles {
            match handle.join() {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::error!(worker = i, error = %e, "worker exited with error");
                    if first_err.is_none() {
                        first_err = Some(e.into());
                    }
                }
                Err(_) => {
                    tracing::error!(worker = i, "worker thread panicked");
                }
            }
        }

        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

#[cfg(all(
    unix,
    not(any(target_os = "solaris", target_os = "illumos", target_os = "cygwin"))
))]
pub use imp::serve_sharded;
