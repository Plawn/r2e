//! [`PreparedApp`]: a fully assembled app plus the serving lifecycle
//! (consumer registration, hooks, single/sharded serve, graceful shutdown).

use super::*;

/// A fully configured R2E app ready to be served.
///
/// Created by [`AppBuilder::prepare()`]. Holds the assembled router, state,
/// lifecycle hooks, and bind address.
///
/// # Hot-reload
///
/// This type enables the Subsecond hot-reload workflow: build the app inside
/// the hot-patched closure with [`AppBuilder::prepare()`], then call
/// [`.run()`](Self::run) to start serving.
pub struct PreparedApp<T: Clone + Send + Sync + 'static> {
    pub(super) router: crate::http::Router,
    pub(super) state: T,
    /// A strong reference to the resolved bean graph, held for the WHOLE
    /// serving lifecycle — see [`run_inner`](Self::run_inner).
    ///
    /// The router's [`GraphKeepAlive`](crate::runtime::layers::GraphKeepAlive) layer
    /// only covers what is derived from the router (request futures, response
    /// bodies), and the router is dropped the moment the serve future
    /// completes — i.e. *before* tracked handles are awaited and before the
    /// shutdown hooks run. Tracked tasks (separate-port gRPC drain,
    /// `spawn_service`, QUIC endpoint drain) carry their own `Arc`; this field
    /// covers the rest of the shutdown phase (`on_stop` hooks, in-flight
    /// WebSocket sessions) so nothing there observes a dead
    /// [`GraphHandle`](crate::plugin::GraphHandle).
    pub(super) graph: Arc<crate::beans::BeanContext>,
    pub(super) addr: String,
    pub(super) startup_hooks: Vec<StartupHook<T>>,
    pub(super) shutdown_hooks: Vec<ShutdownHook<T>>,
    pub(super) drain_hooks: Vec<DrainHook<T>>,
    pub(super) stop_handle: StopHandle,
    pub(super) consumer_registrations: Vec<ConsumerReg<T>>,
    pub(super) post_construct_registrations: Vec<PostConstructReg>,
    pub(super) serve_hooks: Vec<ServeHook>,
    pub(super) plugin_shutdown_hooks: Vec<Box<dyn FnOnce() + Send>>,
    /// Single ordered async-shutdown list, assembled once at build time as
    /// plugin async hooks ++ controller `#[pre_destroy]` hooks ++ bean
    /// `#[pre_destroy]` disposers (each disposer group already in reverse
    /// registration order). Drained in order during the async shutdown phase, so
    /// a controller disposes before the beans it injected.
    pub(super) async_shutdown_hooks: Vec<crate::plugin::AsyncShutdownHook>,
    pub(super) plugin_data: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    pub(super) shutdown_grace_period: Option<Duration>,
    pub(super) tcp_nodelay: bool,
    /// Parsed `server.workers` config. `Ok(None)` → single-listener (default).
    /// `Ok(Some(n))` → SO_REUSEPORT sharded serving with `n` workers.
    /// `Err(msg)` → invalid config value, surfaced as an error at `run()` time.
    pub(super) workers: Result<Option<usize>, String>,
    /// Per-worker service factories ([`AppBuilder::per_worker_service`]).
    /// Non-empty requires the sharded strategy; checked at `run()`.
    pub(super) per_worker_services: Vec<crate::runtime::worker::PerWorkerServiceFactory>,
    #[cfg(feature = "quic")]
    pub(super) quic_server_config:
        Option<(std::net::SocketAddr, r2e_http::quic::quinn::ServerConfig)>,
}

/// Error returned by `run()` when a per-worker service is registered but the
/// app is not serving sharded (`server.workers` absent, hot-reload, or an
/// explicit listener).
pub const PER_WORKER_REQUIRES_SHARDING_MSG: &str =
    "per_worker_service() is registered but server.workers is not set: per-worker \
     services need SO_REUSEPORT sharded serving (set server.workers to N or \"per-core\")";

/// Internal serving strategy chosen by [`PreparedApp::run`].
///
/// The two variants share the entire lifecycle in
/// [`PreparedApp::run_inner`]; only the bind-and-serve middle section differs.
enum ServeStrategy {
    /// Single listener on the caller's runtime (default behavior, unchanged).
    Single(crate::rt::TcpListener),
    /// SO_REUSEPORT sharded serving: `workers` worker threads, each with its
    /// own `current_thread` runtime and listener on the bound address (first
    /// candidate from `addrs` that binds).
    // Under dev-reload the constructing path (`run_sharded`) is compiled out
    // (sharding + hot-reload is unsupported), so the variant is never built.
    #[cfg_attr(feature = "dev-reload", allow(dead_code))]
    Sharded {
        #[allow(dead_code)]
        addrs: Vec<std::net::SocketAddr>,
        #[allow(dead_code)]
        workers: usize,
    },
}

impl<T: Clone + Send + Sync + 'static> PreparedApp<T> {
    /// Access the assembled router for inspection or testing.
    pub fn router(&self) -> &crate::http::Router {
        &self.router
    }

    /// Mutable access to the router (e.g., for adding test-only routes).
    pub fn router_mut(&mut self) -> &mut crate::http::Router {
        &mut self.router
    }

    /// The application state.
    pub fn state(&self) -> &T {
        &self.state
    }

    /// The bind address.
    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// Whether TCP_NODELAY is enabled for accepted connections.
    pub fn tcp_nodelay(&self) -> bool {
        self.tcp_nodelay
    }

    /// A handle that stops the server programmatically.
    ///
    /// Calling [`StopHandle::stop`] triggers the same graceful shutdown as an
    /// OS signal; the [`run()`](Self::run) future resolves once the drain
    /// completes. The handle is `Clone` — grab it before spawning `run()`:
    ///
    /// ```ignore
    /// let prepared = app.prepare("127.0.0.1:8080");
    /// let stop = prepared.stop_handle();
    /// let server = r2e::rt::spawn(prepared.run());
    /// stop.stop();
    /// server.await??;
    /// ```
    pub fn stop_handle(&self) -> StopHandle {
        self.stop_handle.clone()
    }

    /// The parsed `server.workers` (SO_REUSEPORT sharding) configuration.
    ///
    /// `Ok(None)` → single-listener serving (default). `Ok(Some(n))` → sharded
    /// serving with `n` worker threads. `Err(msg)` → the config value was
    /// invalid (e.g. `0` or an unknown string); this error is returned by
    /// [`run()`](Self::run).
    pub fn workers(&self) -> Result<Option<usize>, &str> {
        self.workers.as_ref().copied().map_err(|s| s.as_str())
    }

    /// Start listening and serving requests.
    ///
    /// Registers event consumers, runs startup hooks, binds the TCP listener,
    /// and serves with graceful shutdown. After shutdown, runs plugin and user
    /// shutdown hooks.
    pub async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        // Resolve the `server.workers` config; an invalid value is a hard error.
        let workers = self.workers.clone()?;
        // Per-worker services need worker runtimes to live on; the
        // single-listener path (and the hot-reload path, which forces it) has
        // none. Never fall back silently — the `!Send` ownership promise would
        // be broken on the multi-thread runtime.
        if !self.per_worker_services.is_empty() {
            if workers.is_none() {
                return Err(PER_WORKER_REQUIRES_SHARDING_MSG.into());
            }
            #[cfg(feature = "dev-reload")]
            {
                return Err(format!(
                    "{PER_WORKER_REQUIRES_SHARDING_MSG} (the `dev-reload` feature forces \
                     single-listener serving)"
                )
                .into());
            }
        }

        match workers {
            // Sharded SO_REUSEPORT serving requested.
            Some(n) => {
                // Hot-reload + sharding is unsupported in v1: the dev-reload
                // listener-caching path bypasses sharding entirely.
                #[cfg(feature = "dev-reload")]
                {
                    let _ = n; // sharding ignored under hot-reload
                    tracing::warn!(
                        "server.workers is set but the `dev-reload` feature is active; \
                         SO_REUSEPORT sharding is ignored (unsupported with hot-reload). \
                         Serving with a single listener."
                    );
                    let listener = crate::runtime::dev::get_or_bind_listener(&self.addr)?;
                    self.run_inner(ServeStrategy::Single(listener)).await
                }
                #[cfg(not(feature = "dev-reload"))]
                {
                    self.run_sharded(n).await
                }
            }
            // Default: single listener on the caller's runtime — unchanged.
            None => {
                #[cfg(feature = "dev-reload")]
                let listener = crate::runtime::dev::get_or_bind_listener(&self.addr)?;
                #[cfg(not(feature = "dev-reload"))]
                let listener = crate::rt::bind_tcp(&self.addr).await?;
                self.run_inner(ServeStrategy::Single(listener)).await
            }
        }
    }

    /// Sharded SO_REUSEPORT serving. Resolves the bind address once, then
    /// delegates to [`run_inner`](Self::run_inner) with the sharded strategy.
    #[cfg(not(feature = "dev-reload"))]
    async fn run_sharded(self, workers: usize) -> Result<(), Box<dyn std::error::Error>> {
        #[cfg(all(
            unix,
            not(any(target_os = "solaris", target_os = "illumos", target_os = "cygwin"))
        ))]
        {
            // Resolve the address once on the main runtime (async DNS — never
            // blocking std DNS on an async thread). All candidates are kept:
            // the sharded path tries each in order, like `bind_tcp` does.
            let addrs = crate::rt::lookup_host(&self.addr).await?;
            self.run_inner(ServeStrategy::Sharded { addrs, workers })
                .await
        }
        #[cfg(not(all(
            unix,
            not(any(target_os = "solaris", target_os = "illumos", target_os = "cygwin"))
        )))]
        {
            let _ = workers;
            Err(crate::runtime::sharded::UNSUPPORTED_PLATFORM_MSG.into())
        }
    }

    /// Like [`run()`](Self::run) but with a pre-bound listener.
    ///
    /// This is useful for hot-reload: bind the listener once in setup,
    /// and reuse it across hot-patches so we never fight port conflicts.
    pub async fn run_with_listener(
        self,
        listener: crate::rt::TcpListener,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Sharding is unsupported on the explicit-listener path: the caller
        // owns the (single) listener. If `server.workers` was configured, warn
        // and proceed single-listener.
        if matches!(self.workers, Ok(Some(_))) {
            tracing::warn!(
                "server.workers is set but run_with_listener was called with an \
                 explicit listener; SO_REUSEPORT sharding is ignored. Serving with \
                 the provided single listener."
            );
        }
        if !self.per_worker_services.is_empty() {
            return Err(format!(
                "{PER_WORKER_REQUIRES_SHARDING_MSG} (run_with_listener always serves \
                 single-listener)"
            )
            .into());
        }
        self.run_inner(ServeStrategy::Single(listener)).await
    }

    /// Shared serving core for both single-listener and sharded strategies.
    ///
    /// Owns the full lifecycle: consumer registration, serve/startup hooks,
    /// QUIC spawn, shutdown-future composition, the serve call (single or
    /// sharded), QUIC drain, and the shutdown phase. Only the "bind + serve"
    /// middle differs between strategies.
    async fn run_inner(
        mut self,
        strategy: ServeStrategy,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // ── Serve-scope graph ownership ─────────────────────────────────────
        // The graph has three owners, each covering what the others cannot.
        // NOT a chain of trust in one exit path — every owner stands alone:
        //
        //   1. the router's `GraphKeepAlive` layer — request futures and
        //      response bodies (they outlive the service value, and the body
        //      outlives the head);
        //   2. each TRACKED TASK — `ServeContext::track`, `spawn_service`, the
        //      scheduler driver and the QUIC drain all spawn through
        //      `ServiceHandles::spawn_owning`, which moves an `Arc` INTO the
        //      task. Every exit this function controls cancels the token and
        //      joins those handles (normal shutdown below, and both aborts via
        //      `abort_started_work`), but the join is still best-effort: an
        //      elapsed `shutdown_grace_period` drops the join futures, and a
        //      dropped `run()` future (an `r2e dev` hot patch) joins nothing at
        //      all. Ownership inside the task is what makes those paths sound;
        //   3. this local — moved out of `self` so one named binding, not an
        //      incidental struct field, governs the lifetime. It spans the
        //      whole normal lifecycle: serve future, tracked-handle join, user
        //      shutdown hooks. It is the belt over (1)/(2): whatever the
        //      shutdown phase itself touches (`on_stop` hooks resolving through
        //      a `GraphHandle`, an in-flight WebSocket session) sees a live
        //      graph until `run()` returns.
        //
        // MUST: work that is neither derived from the router nor spawned
        // through the tracked lane is outside all three owners. Two known
        // classes, both bean-owned rather than serve-hook-owned: per-emit
        // event-bus handler dispatch (`LocalEventBus`/backend pollers spawn
        // their own tasks) and `PoolExecutor` jobs submitted directly. Both
        // capture their state strongly; only a body resolving through a
        // `GraphHandle` can observe `None`, and only after the last owner is
        // gone.
        //
        // Residual window, derived — not assumed: a WebSocket session still
        // running AFTER `run()` returns. On upgrade, hyper's
        // `UpgradeableConnection::poll` hands the IO to the upgrade and
        // immediately returns `Ready(Ok(()))`, so the connection counts as
        // finished and axum's `on_upgrade` task (detached, spawned by axum itself, empty
        // 101 body) is watched by neither graceful shutdown nor the tracked
        // set. In a normal binary that window closes immediately (the runtime
        // is dropped right after `run()`, taking detached tasks with it); an
        // embedder that keeps the runtime alive past `run()` must resolve what
        // a session needs BEFORE its socket loop. See `docs/claude/plugins.md`
        // § "The graph outlives the router".
        let serve_scope_graph = self.graph;

        #[cfg(feature = "dev-reload")]
        let skip_lifecycle = crate::runtime::dev::is_lifecycle_initialized();
        #[cfg(not(feature = "dev-reload"))]
        let skip_lifecycle = false;

        // Cancelled when graceful shutdown begins (after drain hooks). Serve
        // hooks receive it via `ServeContext`; the HTTP/QUIC/sharded serving
        // paths observe it as their graceful-shutdown signal.
        //
        // Not a fresh token: this is the app's shutdown ROOT, get-or-inserted
        // time (`ShutdownRoot` in `plugin_data`) because `spawn_service` has to
        // derive its per-service child tokens from it before serving starts.
        // Cancelling here therefore reaches every service task as well.
        let cancel_token = shutdown_root(&mut self.plugin_data);

        // Cancel-on-any-exit, armed BEFORE the serve hooks that spawn tracked
        // tasks. A tracked task is *supposed* to stop when this token fires
        // (that is the contract `ServeContext::shutdown_token` states), so any
        // path that leaves `run_inner` without firing it strands the task
        // forever — it keeps its port, and since round 4 it also keeps the
        // graph alive. The two known aborts (startup-hook `Err`, serve error)
        // cancel explicitly below so they can also DRAIN; this guard is the
        // belt that keeps the invariant true for any future early return —
        // including a panic unwinding out of a hook.
        let _boot_cancel_guard = cancel_token.clone().drop_guard();

        // Plugin sync shutdown hooks cancel the private tokens handed to
        // `spawn_service` tasks, so both the normal shutdown future and the
        // abort paths need them — and neither may run them twice. A run-once
        // cell shared by both is the whole mechanism.
        let plugin_shutdown_hooks = PluginShutdownCell::new(self.plugin_shutdown_hooks);

        // Get-or-insert the shared post-drain handle collector BEFORE serve
        // hooks run: hooks `track()` into it via `ServeContext`, and it must
        // be the same instance the shutdown phase drains (spawn_service
        // inserts it at registration time, but only when used).
        let service_handles = self
            .plugin_data
            .entry(TypeId::of::<ServiceHandles>())
            .or_insert_with(|| Box::new(ServiceHandles::default()))
            .downcast_ref::<ServiceHandles>()
            .expect("ServiceHandles type mismatch in plugin_data")
            .clone();

        if !skip_lifecycle {
            // Controller-core `#[post_construct]` hooks run before consumers
            // (mirroring bean post_construct at `build_state`, before
            // subscribers). A failure aborts startup.
            for pc in self.post_construct_registrations {
                pc.await.map_err(|e| -> Box<dyn std::error::Error> { e })?;
            }

            // Register event consumers
            for reg in self.consumer_registrations {
                reg(self.state.clone()).await;
            }

            // Call serve hooks (e.g., scheduler starts tasks).
            //
            // Each hook receives a `ServeContext`: a clone of the shared
            // `TaskRegistryHandle` (Arc-backed) to drain the tasks it owns,
            // the app shutdown token, and a `track()` collector for tasks
            // whose drain must be awaited at shutdown. Multiple hooks can
            // share the registry: scheduler calls `take_all()` or
            // `take_of::<ScheduledTaskMarker>()`, other subsystems pick their
            // own tagged subset, and absent subsystems observe no tasks.
            let task_registry = self
                .plugin_data
                .get(&TypeId::of::<TaskRegistryHandle>())
                .and_then(|d| d.downcast_ref::<TaskRegistryHandle>())
                .cloned()
                .unwrap_or_default();
            for hook in self.serve_hooks {
                hook(ServeContext {
                    tasks: task_registry.clone(),
                    shutdown: cancel_token.clone(),
                    handles: service_handles.clone(),
                    graph: Arc::clone(&serve_scope_graph),
                });
            }

            // Run startup hooks. They run AFTER the serve hooks, so by now
            // tracked tasks may already be listening on ports and holding the
            // graph; an `Err` here must therefore not just return — it has to
            // wind that work down first (cancel + drain, below).
            let mut startup_error = None;
            for hook in self.startup_hooks {
                if let Err(e) = hook(self.state.clone()).await {
                    startup_error = Some(e);
                    break;
                }
            }
            if let Some(e) = startup_error {
                abort_started_work(
                    &cancel_token,
                    &plugin_shutdown_hooks,
                    &service_handles,
                    self.shutdown_grace_period,
                    "startup hook failed",
                )
                .await;
                return Err(e);
            }

            #[cfg(feature = "dev-reload")]
            crate::runtime::dev::mark_lifecycle_initialized();
        } else {
            tracing::debug!("dev-reload: skipping consumers, serve hooks, and startup hooks");
        }

        // Compose the shutdown future handed to `with_graceful_shutdown`.
        // When the OS signal (or a programmatic `StopHandle::stop`) arrives:
        // 1. user drain hooks are awaited — the server is still accepting and
        //    serving normally (readiness flips, LB deregistration waits);
        // 2. plugin shutdown hooks fire (they cancel tokens handed to
        //    spawn_service tasks) and plugin async shutdown hooks are awaited,
        //    BEFORE the HTTP server starts draining — background tasks see the
        //    cancel signal while in-flight HTTP requests still get to finish;
        // 3. the shared token is cancelled and the listener stops accepting.
        // Hot-patch replaces the previous server future by dropping it, so
        // that future never reaches graceful shutdown. The currently active
        // cycle must therefore retain its shutdown hooks even when startup
        // lifecycle was skipped; otherwise the first no-op patch permanently
        // loses every `#[pre_destroy]` disposer.
        let plugin_hooks_for_shutdown = plugin_shutdown_hooks.clone();
        let async_shutdown_hooks = self.async_shutdown_hooks;
        let drain_hooks = self.drain_hooks;
        let state_for_drain = self.state.clone();
        let stop_handle = self.stop_handle.clone();

        // Spawn the QUIC/HTTP3 endpoint (if configured) before the TCP server.
        // In dev-reload mode, the endpoint is cached so the UDP socket
        // survives across hot-patches without port conflicts. It is spawned
        // through the tracked set (like gRPC / spawn_service), so the QUIC
        // drain is awaited in the shutdown phase — bounded by
        // `shutdown_grace_period` — and the task owns the graph while it runs.
        #[cfg(feature = "quic")]
        if let Some(quic_task) = self
            .quic_server_config
            .take()
            .and_then(|(addr, server_config)| {
                let router = self.router.clone();
                let token = cancel_token.clone();

                #[cfg(feature = "dev-reload")]
                let endpoint_result =
                    crate::runtime::dev::get_or_bind_quic_endpoint(addr, server_config);
                #[cfg(not(feature = "dev-reload"))]
                let endpoint_result =
                    crate::http::quic::quinn::Endpoint::server(server_config, addr)
                        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) });

                match endpoint_result {
                    Ok(endpoint) => {
                        #[cfg(not(feature = "dev-reload"))]
                        let ep_for_close = endpoint.clone();
                        Some(async move {
                            if let Err(e) = crate::http::quic::serve_h3_with_endpoint(
                                router,
                                endpoint,
                                token.cancelled(),
                            )
                            .await
                            {
                                tracing::error!(error = %e, "QUIC/HTTP3 server error");
                            }
                            #[cfg(not(feature = "dev-reload"))]
                            {
                                ep_for_close.close(0u32.into(), b"shutdown");
                                ep_for_close.wait_idle().await;
                            }
                        })
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to bind QUIC endpoint");
                        None
                    }
                }
            })
        {
            service_handles.spawn_owning(Arc::clone(&serve_scope_graph), quic_task);
        }

        let cancel_for_shutdown = cancel_token.clone();
        let shutdown_future = async move {
            // Cancel-on-drop: the token must fire even if a drain or plugin
            // hook panics (in the sharded path this future runs as a spawned
            // task, where a panic is swallowed — without the guard the
            // workers would never see the cancellation and run() would hang
            // forever).
            let _cancel_guard = cancel_for_shutdown.drop_guard();
            crate::rt::select! {
                _ = crate::rt::shutdown_signal() => {}
                _ = stop_handle.stopped() => {
                    tracing::info!("Programmatic stop requested, starting graceful shutdown");
                }
            }
            for hook in drain_hooks {
                hook(state_for_drain.clone()).await;
            }
            plugin_hooks_for_shutdown.fire();
            // Ordered async shutdown: plugin async hooks, then controller
            // `#[pre_destroy]` hooks, then bean `#[pre_destroy]` disposers
            // (assembled in that order at build time).
            for hook in async_shutdown_hooks {
                hook().await;
            }
            // `_cancel_guard` drops here and cancels the token.
        };

        // ── Serve (single-listener or sharded) ──────────────────────────────
        // Only this middle section differs between strategies; the lifecycle
        // start above and the shutdown phase below are shared.
        // `Send + Sync` on purpose: the value stays live across the abort
        // `await` below, so a bare `dyn Error` would make `run()` a non-Send
        // future. Every arm's error already is Send + Sync; the widening to
        // `Box<dyn Error>` happens on return.
        let serve_result: Result<(), Box<dyn std::error::Error + Send + Sync>> = match strategy {
            ServeStrategy::Single(listener) => {
                info!(addr = %self.addr, "R2E server listening");
                let svc = self
                    .router
                    .into_make_service_with_connect_info::<std::net::SocketAddr>();
                if self.tcp_nodelay {
                    use crate::http::ListenerExt as _;
                    crate::http::serve(
                        listener.tap_io(|stream| {
                            if let Err(e) = stream.set_nodelay(true) {
                                tracing::warn!(error = %e, "failed to set TCP_NODELAY on accepted connection");
                            }
                        }),
                        svc,
                    )
                    .with_graceful_shutdown(shutdown_future)
                    .await
                    .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })
                } else {
                    crate::http::serve(listener, svc)
                        .with_graceful_shutdown(shutdown_future)
                        .await
                        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })
                }
            }
            #[cfg(all(
                unix,
                not(any(target_os = "solaris", target_os = "illumos", target_os = "cygwin"))
            ))]
            ServeStrategy::Sharded { addrs, workers } => {
                // Drive the shutdown future on the main runtime: it awaits the
                // OS signal, fires plugin shutdown hooks, then cancels the
                // shared token. Each worker observes a child token's
                // cancellation as its graceful-shutdown signal.
                let shutdown_handle = crate::rt::spawn(shutdown_future);

                let router = self.router.clone();
                let tcp_nodelay = self.tcp_nodelay;
                let cancel_for_workers = cancel_token.clone();
                let per_worker_services = self.per_worker_services.clone();
                // Capture the main (multi-thread) runtime handle as the control
                // plane. Worker threads register it so that background work
                // initiated from request handlers (and lazy-bean first-touch)
                // runs here, not on the workers' current_thread runtimes.
                let control_plane = crate::rt::current_handle();
                if !control_plane.is_multi_thread() {
                    // A current_thread control plane mostly works, but a
                    // worker-side lazy first-touch would block the worker on a
                    // runtime that may itself be busy — sharding is designed
                    // for a multi-thread main runtime.
                    tracing::warn!(
                        "server.workers is set but run() is driven by a \
                         non-multi-thread runtime; the control plane should be \
                         a multi-thread runtime (use #[r2e::main])"
                    );
                }
                // `serve_sharded` blocks the calling thread joining the worker
                // threads, so run it on a blocking task to avoid stalling the
                // main runtime (which must keep driving the shutdown future).
                let join = crate::rt::spawn_blocking(move || {
                    crate::runtime::sharded::serve_sharded(
                        router,
                        &addrs,
                        workers,
                        tcp_nodelay,
                        control_plane,
                        cancel_for_workers,
                        &per_worker_services,
                    )
                })
                .await;

                // Ensure the shutdown future's task is wound down (it has
                // already fired by the time workers exited, since workers only
                // exit on cancellation).
                shutdown_handle.abort();

                match join {
                    Ok(res) => res,
                    Err(e) => Err(format!("sharded serve task failed: {e}").into()),
                }
            }
            #[cfg(not(all(
                unix,
                not(any(target_os = "solaris", target_os = "illumos", target_os = "cygwin"))
            )))]
            ServeStrategy::Sharded { .. } => {
                Err(crate::runtime::sharded::UNSUPPORTED_PLATFORM_MSG.into())
            }
        };
        // A serve error is the second abort path: the shutdown future was
        // dropped mid-`select!` (or aborted, in the sharded path), so the user
        // drain hooks and the async disposers never ran — but the tracked tasks
        // it was supposed to stop are running. Same treatment as an aborted
        // startup: signal, then drain, then return the error.
        if let Err(e) = serve_result {
            abort_started_work(
                &cancel_token,
                &plugin_shutdown_hooks,
                &service_handles,
                self.shutdown_grace_period,
                "serve failed",
            )
            .await;
            return Err(e);
        }

        // After HTTP drain completes: await tracked JobHandles (spawn_service
        // tasks, serve-hook tasks registered via `ServeContext::track` such
        // as the gRPC server drain, the scheduler driver, and the QUIC endpoint
        // drain), then run user shutdown hooks. Both phases together are
        // bounded by `shutdown_grace_period` if set.
        let state_for_shutdown = self.state.clone();
        let shutdown_hooks = self.shutdown_hooks;
        let shutdown_phase = async move {
            drain_tracked_handles(&service_handles).await;

            for hook in shutdown_hooks {
                hook(state_for_shutdown.clone()).await;
            }
        };

        if let Some(grace) = self.shutdown_grace_period {
            if crate::rt::timeout(grace, shutdown_phase).await.is_err() {
                tracing::warn!(
                    grace_secs = grace.as_secs(),
                    "Shutdown grace period elapsed; some background tasks did not finish in time"
                );
            }
        } else {
            shutdown_phase.await;
        }

        // Explicit end of the serve-scope ownership window. Note what this
        // does NOT claim: that everything detached has finished. Tasks the
        // grace period abandoned may still be running — they carry their own
        // `Arc` (`ServiceHandles::spawn_owning`), so this drop is not the last
        // one and the graph lives until the last of them ends.
        drop(serve_scope_graph);

        info!("R2E server stopped");
        Ok(())
    }
}

/// Run-once holder for the plugin *sync* shutdown hooks.
///
/// Those hooks are pure signals: they cancel the per-service tokens handed to
/// `spawn_service` tasks (see [`AppBuilder::spawn_service`]) *early* — before
/// the HTTP drain — which is the ordering guarantee the docs make. They are no
/// longer what keeps those tasks from being stranded: since the per-service
/// token is a child of the app shutdown root (see `ShutdownRoot`), cancelling
/// the root reaches them even when no hook runs at all.
///
/// Both the normal shutdown future and the abort paths of `run_inner` fire the
/// cell, and a `FnOnce` cannot be run twice, so the list lives behind a shared
/// `Option` and each hook is taken out of it exactly once.
#[derive(Clone)]
pub(super) struct PluginShutdownCell(Arc<Mutex<Option<SyncShutdownHooks>>>);

/// The plugin sync shutdown hooks, as `PreparedApp` receives them.
type SyncShutdownHooks = Vec<Box<dyn FnOnce() + Send>>;

impl PluginShutdownCell {
    fn new(hooks: SyncShutdownHooks) -> Self {
        Self(Arc::new(Mutex::new(Some(hooks))))
    }

    /// Run the hooks if nobody has yet. Subsequent calls are no-ops.
    ///
    /// Two failure modes are handled explicitly, because a shutdown signal that
    /// silently stops halfway strands background tasks forever:
    ///
    /// - hooks are taken ONE AT A TIME, not as a whole vector: if a hook panics
    ///   (or this call unwinds for any other reason) the ones not yet run stay
    ///   in the cell, so a later `fire()` — the abort path, or the drop-guard
    ///   ordering below — still delivers them;
    /// - each hook runs inside `catch_unwind`, so one bad plugin cannot stop
    ///   the rest of the list from being signalled.
    fn pop(&self) -> Option<Box<dyn FnOnce() + Send>> {
        // The lock is released before running a hook: a hook is user/plugin
        // code and must never observe (or deadlock on) this mutex.
        let mut guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        // Front-first: registration order is the documented firing order.
        match guard.as_mut() {
            Some(hooks) if !hooks.is_empty() => Some(hooks.remove(0)),
            _ => None,
        }
    }

    fn fire(&self) {
        while let Some(hook) = self.pop() {
            if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(hook)) {
                // The default panic hook already printed the location; repeat
                // the message here so the shutdown log alone says which hook.
                let msg = payload
                    .downcast_ref::<&'static str>()
                    .copied()
                    .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("<non-string panic payload>");
                tracing::error!(panic = %msg, "plugin shutdown hook panicked; continuing shutdown");
            }
        }
        // Mark the cell as spent so a later `fire()` is a cheap no-op.
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// Await every tracked task handle, logging join failures.
///
/// The tracked set is the union of `spawn_service` tasks, `ServeContext::track`
/// tasks (gRPC server drain, scheduler driver, tenant sweeper, …) and the QUIC
/// endpoint drain. Callers bound this by `shutdown_grace_period`; it never
/// bounds itself.
async fn drain_tracked_handles(handles: &ServiceHandles) {
    let handles = handles.drain();
    if handles.is_empty() {
        return;
    }
    tracing::info!(count = handles.len(), "Awaiting background tasks to finish");
    for h in handles {
        if let Err(e) = h.await {
            if e.is_panic() {
                tracing::warn!(error = %e, "background task panicked");
            } else if !e.is_cancelled() {
                tracing::warn!(error = %e, "background task join error");
            }
        }
    }
}

/// Wind down work already started by the serve hooks when the boot aborts.
///
/// Called on the two error exits of `run_inner` that happen after serve hooks
/// ran (a startup hook returning `Err`, and a serve error). Order mirrors the
/// normal shutdown: signal first (app token, then the plugin hooks that cancel
/// `spawn_service`'s private tokens), then await the tracked handles under the
/// same `shutdown_grace_period` policy.
///
/// What it deliberately does NOT run: user `on_drain`/`on_stop` hooks and the
/// async disposers (`#[pre_destroy]`, plugin `on_shutdown_async`). Those are
/// the shutdown of a *running* app; on an aborted boot the app never served,
/// and firing them would hand user code a half-initialized world. The caller
/// still returns the original error, which is what aborts the process.
async fn abort_started_work(
    cancel: &CancelToken,
    plugin_hooks: &PluginShutdownCell,
    handles: &ServiceHandles,
    grace: Option<Duration>,
    reason: &'static str,
) {
    cancel.cancel();
    plugin_hooks.fire();

    // MUST: nothing here awaits work that is not in `handles`. A task detached
    // with a bare `rt::spawn` (rather than `ServeContext::track`) is invisible
    // to this drain — that is why every in-tree plugin routes serve-time work
    // through `track`.
    let drain = drain_tracked_handles(handles);
    match grace {
        Some(grace) => {
            if crate::rt::timeout(grace, drain).await.is_err() {
                tracing::warn!(
                    reason,
                    grace_secs = grace.as_secs(),
                    "Aborting boot: grace period elapsed before background tasks finished"
                );
            }
        }
        None => drain.await,
    }
    tracing::warn!(reason, "R2E boot aborted; background tasks wound down");
}
