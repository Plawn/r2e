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
    pub(super) addr: String,
    pub(super) startup_hooks: Vec<StartupHook<T>>,
    pub(super) shutdown_hooks: Vec<ShutdownHook<T>>,
    pub(super) consumer_registrations: Vec<ConsumerReg<T>>,
    pub(super) serve_hooks: Vec<ServeHook>,
    pub(super) plugin_shutdown_hooks: Vec<Box<dyn FnOnce() + Send>>,
    pub(super) plugin_async_shutdown_hooks: Vec<crate::plugin::AsyncShutdownHook>,
    pub(super) plugin_data: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    pub(super) shutdown_grace_period: Option<Duration>,
    pub(super) tcp_nodelay: bool,
    /// Parsed `server.workers` config. `Ok(None)` → single-listener (default).
    /// `Ok(Some(n))` → SO_REUSEPORT sharded serving with `n` workers.
    /// `Err(msg)` → invalid config value, surfaced as an error at `run()` time.
    pub(super) workers: Result<Option<usize>, String>,
    #[cfg(feature = "quic")]
    pub(super) quic_server_config:
        Option<(std::net::SocketAddr, r2e_http::quic::quinn::ServerConfig)>,
}

/// Internal serving strategy chosen by [`PreparedApp::run`].
///
/// The two variants share the entire lifecycle in
/// [`PreparedApp::run_inner`]; only the bind-and-serve middle section differs.
enum ServeStrategy {
    /// Single listener on the caller's runtime (default behavior, unchanged).
    Single(tokio::net::TcpListener),
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
                    let listener = crate::dev::get_or_bind_listener(&self.addr)?;
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
                let listener = crate::dev::get_or_bind_listener(&self.addr)?;
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
            Err(crate::sharded::UNSUPPORTED_PLATFORM_MSG.into())
        }
    }

    /// Like [`run()`](Self::run) but with a pre-bound listener.
    ///
    /// This is useful for hot-reload: bind the listener once in setup,
    /// and reuse it across hot-patches so we never fight port conflicts.
    pub async fn run_with_listener(
        self,
        listener: tokio::net::TcpListener,
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
        self.run_inner(ServeStrategy::Single(listener)).await
    }

    /// Shared serving core for both single-listener and sharded strategies.
    ///
    /// Owns the full lifecycle: consumer registration, serve/startup hooks,
    /// QUIC spawn, shutdown-future composition, the serve call (single or
    /// sharded), QUIC drain, and the shutdown phase. Only the "bind + serve"
    /// middle differs between strategies.
    async fn run_inner(
        #[cfg_attr(not(feature = "quic"), allow(unused_mut))] mut self,
        strategy: ServeStrategy,
    ) -> Result<(), Box<dyn std::error::Error>> {
        #[cfg(feature = "dev-reload")]
        let skip_lifecycle = crate::dev::is_lifecycle_initialized();
        #[cfg(not(feature = "dev-reload"))]
        let skip_lifecycle = false;

        if !skip_lifecycle {
            // Register event consumers
            for reg in self.consumer_registrations {
                reg(self.state.clone()).await;
            }

            // Call serve hooks (e.g., scheduler starts tasks).
            //
            // Each hook receives a clone of the shared `TaskRegistryHandle`
            // (Arc-backed) and drains the tasks it owns. Multiple hooks can
            // share the registry: scheduler calls `take_all()` or
            // `take_of::<ScheduledTaskMarker>()`, other subsystems pick their
            // own tagged subset, and absent subsystems observe no tasks.
            let task_registry = self.plugin_data
                .get(&TypeId::of::<TaskRegistryHandle>())
                .and_then(|d| d.downcast_ref::<TaskRegistryHandle>())
                .cloned()
                .unwrap_or_default();
            for hook in self.serve_hooks {
                hook(task_registry.clone(), CancellationToken::new());
            }

            // Run startup hooks
            for hook in self.startup_hooks {
                hook(self.state.clone())
                    .await
                    .map_err(|e| -> Box<dyn std::error::Error> { e })?;
            }

            #[cfg(feature = "dev-reload")]
            crate::dev::mark_lifecycle_initialized();
        } else {
            tracing::debug!("dev-reload: skipping consumers, serve hooks, and startup hooks");
        }

        // Pull the shared spawn_service JobHandle collector (if any) so we
        // can await tasks after graceful shutdown.
        let service_handles = self
            .plugin_data
            .get(&TypeId::of::<ServiceHandles>())
            .and_then(|b| b.downcast_ref::<ServiceHandles>())
            .cloned()
            .unwrap_or_default();

        // Compose the shutdown future handed to `with_graceful_shutdown`.
        // When the OS signal arrives, fire plugin shutdown hooks (which
        // cancel tokens handed to spawn_service tasks) BEFORE letting the
        // HTTP server start draining. This way background tasks see the
        // cancel signal while in-flight HTTP requests still get to finish.
        let (plugin_shutdown_hooks, plugin_async_shutdown_hooks) = if skip_lifecycle {
            (Vec::new(), Vec::new())
        } else {
            (self.plugin_shutdown_hooks, self.plugin_async_shutdown_hooks)
        };
        let cancel_token = CancellationToken::new();

        // Spawn the QUIC/HTTP3 endpoint (if configured) before the TCP server.
        // In dev-reload mode, the endpoint is cached so the UDP socket
        // survives across hot-patches without port conflicts.
        #[cfg(feature = "quic")]
        let quic_handle = self.quic_server_config.take().and_then(|(addr, server_config)| {
            let router = self.router.clone();
            let token = cancel_token.clone();

            #[cfg(feature = "dev-reload")]
            let endpoint_result = crate::dev::get_or_bind_quic_endpoint(addr, server_config);
            #[cfg(not(feature = "dev-reload"))]
            let endpoint_result = crate::http::quic::quinn::Endpoint::server(server_config, addr)
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) });

            match endpoint_result {
                Ok(endpoint) => {
                    #[cfg(not(feature = "dev-reload"))]
                    let ep_for_close = endpoint.clone();
                    Some(crate::rt::spawn(async move {
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
                    }))
                }
                Err(e) => {
                    tracing::error!(error = %e, "Failed to bind QUIC endpoint");
                    None
                }
            }
        });

        let cancel_for_shutdown = cancel_token.clone();
        let shutdown_future = async move {
            crate::rt::shutdown_signal().await;
            for hook in plugin_shutdown_hooks {
                hook();
            }
            for hook in plugin_async_shutdown_hooks {
                hook().await;
            }
            cancel_for_shutdown.cancel();
        };

        // ── Serve (single-listener or sharded) ──────────────────────────────
        // Only this middle section differs between strategies; the lifecycle
        // start above and the shutdown phase below are shared.
        let serve_result: Result<(), Box<dyn std::error::Error>> = match strategy {
            ServeStrategy::Single(listener) => {
                info!(addr = %self.addr, "R2E server listening");
                let svc = self.router
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
                    .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })
                } else {
                    crate::http::serve(listener, svc)
                        .with_graceful_shutdown(shutdown_future)
                        .await
                        .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })
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
                // Capture the main (multi-thread) runtime handle as the control
                // plane. Worker threads register it so that background work
                // initiated from request handlers (and lazy-bean first-touch)
                // runs here, not on the workers' current_thread runtimes.
                let control_plane = crate::rt::current_handle();
                if control_plane.runtime_flavor()
                    != tokio::runtime::RuntimeFlavor::MultiThread
                {
                    // A current_thread control plane mostly works, but a
                    // worker-side lazy first-touch would block the worker on a
                    // runtime that may itself be busy — sharding is designed
                    // for a multi-thread main runtime.
                    tracing::warn!(
                        "server.workers is set but run() is driven by a \
                         non-multi-thread runtime; the control plane should be \
                         a multi-thread runtime (use #[tokio::main])"
                    );
                }
                // `serve_sharded` blocks the calling thread joining the worker
                // threads, so run it on a blocking task to avoid stalling the
                // main runtime (which must keep driving the shutdown future).
                let join = crate::rt::spawn_blocking(move || {
                    crate::sharded::serve_sharded(
                        router,
                        &addrs,
                        workers,
                        tcp_nodelay,
                        control_plane,
                        cancel_for_workers,
                    )
                })
                .await;

                // Ensure the shutdown future's task is wound down (it has
                // already fired by the time workers exited, since workers only
                // exit on cancellation).
                shutdown_handle.abort();

                match join {
                    Ok(res) => res.map_err(|e| -> Box<dyn std::error::Error> { e }),
                    Err(e) => Err(format!("sharded serve task failed: {e}").into()),
                }
            }
            #[cfg(not(all(
                unix,
                not(any(target_os = "solaris", target_os = "illumos", target_os = "cygwin"))
            )))]
            ServeStrategy::Sharded { .. } => {
                Err(crate::sharded::UNSUPPORTED_PLATFORM_MSG.into())
            }
        };
        serve_result?;

        // Wait for QUIC endpoint to drain after TCP server stops.
        #[cfg(feature = "quic")]
        if let Some(handle) = quic_handle {
            if let Err(join_err) = handle.await {
                if join_err.is_panic() {
                    tracing::warn!("QUIC task panicked");
                }
            }
        }

        // After HTTP drain completes: await spawn_service JobHandles with a
        // deadline, then run user shutdown hooks. Both phases together are
        // bounded by `shutdown_grace_period` if set.
        let state_for_shutdown = self.state.clone();
        let shutdown_hooks = self.shutdown_hooks;
        let shutdown_phase = async move {
            let handles = service_handles.drain();
            if !handles.is_empty() {
                tracing::info!(
                    count = handles.len(),
                    "Awaiting spawn_service tasks to finish"
                );
                for h in handles {
                    if let Err(e) = h.await {
                        if e.is_panic() {
                            tracing::warn!(error = %e, "spawn_service task panicked");
                        } else if !e.is_cancelled() {
                            tracing::warn!(error = %e, "spawn_service task join error");
                        }
                    }
                }
            }

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

        info!("R2E server stopped");
        Ok(())
    }
}
