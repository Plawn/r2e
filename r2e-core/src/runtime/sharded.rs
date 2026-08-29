//! SO_REUSEPORT sharded serving — option A of the thread-per-core plan.
//!
//! When `server.workers` is configured, R2E serves HTTP with `N` worker
//! threads, each running its own `current_thread` runtime with its own
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
//! # Worker parking (shutdown)
//!
//! A worker's runtime owns the I/O driver of every socket it accepted —
//! including sockets that were *upgraded* (WebSocket) and are now driven by a
//! task on the control plane. Dropping the worker runtime kills that driver, so
//! an upgraded session would lose its socket mid-shutdown. Workers therefore
//! **park** after their HTTP drain and their per-worker services are down:
//! each reports through [`WorkerPark::drained`] and then waits on
//! [`WorkerPark::release`], which the control plane cancels only once it has
//! joined the tracked handles (the WebSocket sessions among them). See
//! [`crate::builder::WsSessions`] § "Sharded serving".
//!
//! # Lazy beans
//!
//! A lazy bean first touched from within a worker is resolved on the
//! control-plane runtime: because the worker registered the control-plane
//! handle, [`crate::di::lazy`]'s `resolve_lazy_factory` spawns the factory on the
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
    use std::sync::Arc;

    use crate::rt::CancelToken;
    use crate::runtime::ingress::reuseport_tcp;
    use crate::runtime::worker::{
        shutdown_services, start_services, PerWorkerServiceFactory, WorkerContext, WorkerInfo,
        WorkerRole,
    };
    use crate::runtime::worker_set::{WorkerSet, WorkerState};

    /// Sends the worker's startup outcome to the main thread exactly once —
    /// including when the worker thread unwinds before reporting (runtime
    /// build failure, factory panic): the `Drop` impl reports the failure so
    /// the barrier in [`serve_sharded`] never hangs on a dead worker.
    struct ReadyGuard {
        worker: usize,
        tx: Option<std::sync::mpsc::Sender<(usize, Result<(), String>)>>,
    }

    impl ReadyGuard {
        fn report(&mut self, res: Result<(), String>) {
            if let Some(tx) = self.tx.take() {
                let _ = tx.send((self.worker, res));
            }
        }
    }

    impl Drop for ReadyGuard {
        fn drop(&mut self) {
            self.report(Err(format!(
                "worker {} exited before reporting startup (panicked?)",
                self.worker
            )));
        }
    }

    /// Marks the worker `Exited` (or keeps `Failed`) and uninstalls its
    /// [`WorkerInfo`] when the thread returns — including by unwinding.
    struct ExitGuard(Arc<crate::runtime::worker_set::WorkerSlot>);

    impl Drop for ExitGuard {
        fn drop(&mut self) {
            if std::thread::panicking() {
                self.0.fail("worker thread panicked");
            } else if self.0.state() != WorkerState::Failed {
                self.0.set_state(WorkerState::Exited);
            }
            WorkerInfo::uninstall();
        }
    }

    /// Handshake keeping a worker's runtime alive across the control plane's
    /// tracked-handle join.
    ///
    /// A worker sends one `()` on `drained` (and drops the sender) as soon as
    /// its HTTP drain is over and its per-worker services are down, then waits
    /// for `release` before returning from `block_on` — i.e. before its runtime,
    /// and the I/O driver of every socket it accepted, is dropped. The control
    /// plane counts the reports, joins the tracked handles, and only then
    /// cancels `release`.
    ///
    /// `release` is held by the caller through a
    /// [`CancelDropGuard`](crate::rt::CancelDropGuard), so workers are released
    /// even if the shutdown path unwinds or the `run()` future is dropped.
    #[derive(Clone)]
    pub struct WorkerPark {
        /// One `()` per worker, sent once that worker is drained and parked.
        pub drained: crate::rt::sync::mpsc::UnboundedSender<()>,
        /// Cancelled by the control plane once the tracked handles are joined.
        pub release: CancelToken,
    }

    impl WorkerPark {
        /// A handshake nobody is listening to: `release` is already cancelled,
        /// so workers drop their runtime as soon as they are drained, and the
        /// `drained` reports go nowhere. For callers that drive
        /// [`serve_sharded`] directly and own no tracked handles (tests).
        pub fn unparked() -> Self {
            let (drained, _rx) = crate::rt::sync::mpsc::unbounded_channel();
            let release = CancelToken::new();
            release.cancel();
            Self { drained, release }
        }
    }

    /// Serve `router` across `workers` worker threads, each with its own
    /// `current_thread` runtime and `SO_REUSEPORT` listener.
    ///
    /// `addrs` holds the resolved bind-address candidates, in resolver order.
    /// The first listener tries each candidate until one binds (mirroring
    /// [`rt::bind_tcp`](crate::rt::bind_tcp)'s multi-address fallback); the
    /// remaining workers then bind that listener's concrete `local_addr()`.
    /// Going through `local_addr()` also makes port `0` work: the kernel
    /// assigns the ephemeral port once, and every worker shares it.
    ///
    /// `services` are the per-worker service factories (see
    /// [`crate::runtime::worker`]). Each worker runs every factory, in order,
    /// inside its runtime's `LocalSet` **before** accepting connections. Startup
    /// is all-or-nothing across workers: the main thread waits for every
    /// worker to report, then releases them all to serve; if any worker fails,
    /// the shared token is cancelled (every worker unwinds its started
    /// services), all threads are joined, and the error — naming the worker and
    /// the failing service index — is returned.
    ///
    /// Blocks until `cancel_token` is cancelled (each worker observes a child
    /// token via graceful shutdown), then joins all worker threads. Inside a
    /// worker, cancellation drains HTTP first, then shuts down the services in
    /// reverse start order, then **parks** on [`WorkerPark`] until the control
    /// plane has joined the tracked handles, and only then drops the runtime.
    /// The join below therefore returns one `park.release` cancellation later
    /// than the workers' own drain.
    ///
    /// `drain_timeout` ([`AppBuilder::drain_timeout`](crate::builder::AppBuilder::drain_timeout))
    /// bounds each worker's HTTP drain **individually**, measured from the
    /// worker's own cancellation. Since every worker's child token is cancelled
    /// by the same parent, the whole set still finishes within `drain_timeout`
    /// of the shutdown signal — matching the single-listener strategy exactly.
    /// A worker whose drain times out still shuts its per-worker services down.
    ///
    /// `set` is the [`WorkerSet`] the workers report their lifecycle into
    /// (`Starting → Ready → Serving → Draining → ServicesDown → Parked →
    /// Exited`, or `Failed`); it is (re)configured to `workers` slots here.
    ///
    /// Returns the first worker error, if any. Worker panics are logged via
    /// `tracing::error!`.
    // One more than clippy's default: `drain_timeout` joins the existing
    // router/addrs/workers/nodelay/control-plane/token/services set. Bundling
    // them into a struct would only move the same seven values one level down.
    #[allow(clippy::too_many_arguments)]
    pub fn serve_sharded(
        router: crate::http::Router,
        addrs: &[SocketAddr],
        workers: usize,
        tcp_nodelay: bool,
        control_plane: crate::rt::RuntimeHandle,
        cancel_token: CancelToken,
        drain_timeout: Option<std::time::Duration>,
        services: &[PerWorkerServiceFactory],
        park: WorkerPark,
        set: WorkerSet,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        set.configure(workers);
        // Pre-create the listeners on the main thread so that a bind failure
        // surfaces synchronously as a run error (rather than from inside a
        // worker thread). Each worker gets its own SO_REUSEPORT socket.
        let mut last_err: Option<crate::runtime::ingress::AffinityError> = None;
        let mut first_listener = None;
        for candidate in addrs {
            match reuseport_tcp(*candidate) {
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
            // Mirror `rt::bind_tcp`: surface the last bind error.
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
            std_listeners.push(reuseport_tcp(addr)?);
        }

        // Startup barrier: every worker reports (worker, Ok | Err) once its
        // services are up; `start_gate` is cancelled to release them all to
        // serve, or `cancel_token` is cancelled to abort startup.
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<(usize, Result<(), String>)>();
        let start_gate = CancelToken::new();
        let services: Arc<[PerWorkerServiceFactory]> = services.into();

        let mut handles = Vec::with_capacity(workers);
        for (i, std_listener) in std_listeners.into_iter().enumerate() {
            let router = router.clone();
            let child_token = cancel_token.child_token();
            let control_plane = control_plane.clone();
            let services = Arc::clone(&services);
            let start_gate = start_gate.clone();
            let WorkerPark {
                drained: park_drained,
                release: park_release,
            } = park.clone();
            let mut ready = ReadyGuard {
                worker: i,
                tx: Some(ready_tx.clone()),
            };
            let slot = set.slot(i).expect("WorkerSet configured for every worker");
            let handle = std::thread::Builder::new()
                .name(format!("r2e-worker-{i}"))
                .spawn(move || -> Result<(), String> {
                    let _span = tracing::info_span!("r2e_worker", worker = i).entered();
                    let _exit = ExitGuard(Arc::clone(&slot));
                    WorkerInfo::new(i, workers, None, WorkerRole::DataPlane).install();
                    slot.set_state(WorkerState::Starting);
                    // Register the control-plane handle so background work
                    // initiated from request handlers (rt::spawn_ctl) and
                    // lazy-bean first-touch run on the main multi-thread
                    // runtime, not this worker's current_thread runtime.
                    crate::rt::set_control_plane(control_plane);
                    let rt = crate::rt::RuntimeBuilder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| format!("worker {i}: failed to build worker runtime: {e}"))?;
                    // The LocalSet hosts the per-worker services and every
                    // task they `spawn_local`; it is dropped (cancelling any
                    // leftover local task) only after the services have shut
                    // down.
                    let local = crate::rt::LocalSet::new();
                    rt.block_on(local.run_until(async move {
                        // `from_std` must run inside the worker's runtime
                        // context.
                        let listener = crate::rt::TcpListener::from_std(std_listener)
                            .map_err(|e| format!("worker {i}: failed to adopt worker listener: {e}"))?;

                        // ── Per-worker services: start, in order ─────────
                        let ctx = WorkerContext::new(i, workers, None, child_token.clone());
                        let started = match start_services(&ctx, &services).await {
                            Ok(started) => started,
                            Err((k, e)) => {
                                let err = format!(
                                    "worker {i}: per-worker service #{k} failed to start: {e}"
                                );
                                tracing::error!(worker = i, error = %err, "per-worker service startup failed");
                                slot.fail(err.clone());
                                ready.report(Err(err.clone()));
                                return Err(err);
                            }
                        };
                        slot.set_state(WorkerState::Ready);
                        ready.report(Ok(()));
                        // Flip to Draining the instant this worker's token is
                        // cancelled, independently of how long the HTTP drain
                        // takes.
                        let drain_watch = {
                            let slot = Arc::clone(&slot);
                            let token = child_token.clone();
                            crate::rt::spawn_local(async move {
                                token.cancelled().await;
                                if slot.state() == WorkerState::Serving {
                                    slot.set_state(WorkerState::Draining);
                                }
                            })
                        };

                        // ── Barrier: wait for every worker, or abort ─────
                        let released = crate::rt::select! {
                            _ = start_gate.cancelled() => true,
                            _ = child_token.cancelled() => false,
                        };
                        if !released {
                            // Another worker failed to start (or shutdown was
                            // requested before startup completed): unwind
                            // without ever accepting a connection.
                            shutdown_services(i, started).await;
                            slot.set_state(WorkerState::ServicesDown);
                            return Ok(());
                        }
                        slot.set_state(WorkerState::Serving);

                        // ── Serve ────────────────────────────────────────
                        // Wrapped in `bounded_http_drain` exactly like the
                        // single-listener path: the budget starts when this
                        // worker's `child_token` is cancelled — the same
                        // instant its graceful shutdown begins — and on
                        // overflow the serve future is dropped, abandoning
                        // whatever connections this worker still holds.
                        let svc = router.into_make_service_with_connect_info::<SocketAddr>();
                        let shutdown = child_token.clone().cancelled_owned();
                        use std::future::IntoFuture as _;
                        let serve_result = if tcp_nodelay {
                            use crate::http::ListenerExt as _;
                            crate::runtime::drain::bounded_http_drain(
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
                                .into_future(),
                                child_token.clone(),
                                drain_timeout,
                            )
                            .await
                        } else {
                            crate::runtime::drain::bounded_http_drain(
                                crate::http::serve(listener, svc)
                                    .with_graceful_shutdown(shutdown)
                                    .into_future(),
                                child_token.clone(),
                                drain_timeout,
                            )
                            .await
                        };

                        // ── Shutdown: HTTP drained, now the services ─────
                        // Make sure local tasks see cancellation even when the
                        // serve loop ended on an error rather than on the token.
                        child_token.cancel();
                        drain_watch.abort();
                        slot.set_state(WorkerState::Draining);
                        shutdown_services(i, started).await;
                        slot.set_state(WorkerState::ServicesDown);

                        // ── Park: outlive the tracked-handle join ────────
                        // This worker is drained, but its runtime still owns
                        // the I/O driver of every socket it accepted —
                        // including the upgraded WebSocket sessions now
                        // running on the control plane. Report, then stay
                        // inside `block_on` (which keeps that driver turning)
                        // until the control plane has joined them.
                        let _ = park_drained.send(());
                        drop(park_drained);
                        slot.set_state(WorkerState::Parked);
                        park_release.cancelled().await;

                        serve_result.map_err(|e| {
                            let msg = format!("worker {i}: serve error: {e}");
                            slot.fail(msg.clone());
                            msg
                        })
                    }))
                })
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    format!("failed to spawn worker thread {i}: {e}").into()
                })?;
            handles.push((i, handle));
        }
        drop(ready_tx);
        // The workers hold the only remaining senders: once every one of them
        // has either reported or died, the control plane's `recv()` yields
        // `None` instead of hanging.
        drop(park);

        // ── Startup barrier (main thread) ───────────────────────────────────
        // Collect one report per worker. A worker that dies before reporting
        // is reported by its `ReadyGuard`, so this cannot hang.
        let mut startup_err: Option<String> = None;
        for _ in 0..workers {
            match ready_rx.recv() {
                Ok((_, Ok(()))) => {}
                Ok((w, Err(e))) => {
                    tracing::error!(worker = w, error = %e, "worker failed to start");
                    if startup_err.is_none() {
                        startup_err = Some(e);
                    }
                }
                Err(_) => {
                    // Every sender gone without `workers` reports: cannot
                    // happen given the guards, but never hang on it.
                    if startup_err.is_none() {
                        startup_err = Some("a worker vanished during startup".to_string());
                    }
                    break;
                }
            }
        }
        match &startup_err {
            None => {
                start_gate.cancel();
                if services.is_empty() {
                    tracing::info!(%addr, workers, "R2E server listening (sharded, SO_REUSEPORT)");
                } else {
                    tracing::info!(
                        %addr,
                        workers,
                        per_worker_services = services.len(),
                        "R2E server listening (sharded, SO_REUSEPORT)"
                    );
                }
            }
            Some(_) => {
                // Deterministic rollback: every worker unwinds its started
                // services and exits before we return the error.
                cancel_token.cancel();
            }
        }

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

        if let Some(e) = startup_err {
            return Err(e.into());
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
pub use imp::{serve_sharded, WorkerPark};
