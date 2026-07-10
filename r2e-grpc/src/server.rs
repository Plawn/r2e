use r2e_core::{DeferredAction, PluginInstallContext, PreStatePlugin};
use tokio_util::sync::CancellationToken;

use crate::multiplex::MultiplexService;
use crate::registry::GrpcServiceRegistry;

/// Transport mode for the gRPC server.
#[derive(Debug, Clone)]
pub enum GrpcTransport {
    /// Run gRPC on a separate port (e.g., HTTP on :3000, gRPC on :50051).
    SeparatePort(String),
    /// Multiplex gRPC and HTTP on a single port, using content-type detection.
    Multiplexed,
}

/// gRPC server plugin for R2E.
///
/// Install as a `PreStatePlugin` before `build_state()`. The plugin stores a
/// [`GrpcServiceRegistry`] that `register_grpc_service` fills with built
/// services, and drains it once at serve time:
///
/// - **Separate port** ([`GrpcServer::on_port`]): a serve hook spawns a tonic
///   server on the configured address alongside the HTTP server, with
///   graceful shutdown tied to the app's shutdown sequence.
/// - **Multiplexed** ([`GrpcServer::multiplexed`]): the accumulated gRPC
///   routes are wrapped around the HTTP router via [`MultiplexService`], so
///   `content-type: application/grpc*` requests on the HTTP port are served
///   by tonic. gRPC requires HTTP/2; plaintext clients must use h2c prior
///   knowledge (tonic's default), which the HTTP server accepts.
///
/// # Example
///
/// ```ignore
/// use r2e_grpc::GrpcServer;
///
/// AppBuilder::new()
///     .plugin(GrpcServer::on_port("0.0.0.0:50051"))
///     // or: .plugin(GrpcServer::multiplexed())
///     .build_state()
///     .await
///     .register_grpc_service::<UserGrpcService>()
///     .serve("0.0.0.0:3000")
/// ```
pub struct GrpcServer {
    transport: GrpcTransport,
    reflection: bool,
}

impl GrpcServer {
    /// Create a gRPC server plugin that listens on a separate port.
    pub fn on_port(addr: impl Into<String>) -> Self {
        Self {
            transport: GrpcTransport::SeparatePort(addr.into()),
            reflection: false,
        }
    }

    /// Create a gRPC server plugin that multiplexes with HTTP on the same port.
    pub fn multiplexed() -> Self {
        Self {
            transport: GrpcTransport::Multiplexed,
            reflection: false,
        }
    }

    /// Enable gRPC server reflection.
    ///
    /// NOT IMPLEMENTED YET: the flag is accepted but no reflection service is
    /// installed — a warning is logged at install time. Tracked in
    /// `docs/claude/roadmap.md` (serve lifecycle gaps).
    pub fn with_reflection(mut self) -> Self {
        self.reflection = true;
        self
    }
}

impl PreStatePlugin for GrpcServer {
    /// GrpcServer doesn't provide beans — it uses `GrpcMarker` as a placeholder.
    /// The real coordination happens via `GrpcServiceRegistry` in plugin_data.
    type Provided = GrpcMarker;
    type Deps = ();

    fn install(self, (): (), ctx: &mut PluginInstallContext<'_>) -> GrpcMarker {
        if self.reflection {
            tracing::warn!(
                "GrpcServer::with_reflection() is not implemented yet — no \
                 reflection service will be installed"
            );
        }
        let registry = GrpcServiceRegistry::new();
        let transport = self.transport;
        let cancel = CancellationToken::new();
        let cancel_for_shutdown = cancel.clone();

        ctx.add_deferred(DeferredAction::new("GrpcServer", move |dctx| {
            // Store the registry for register_grpc_service to find.
            dctx.store_data(registry.clone());

            match transport {
                GrpcTransport::SeparatePort(addr) => {
                    // Drain the registry when the server starts and spawn the
                    // tonic server next to the HTTP one. Serve hooks run before
                    // the HTTP listener binds, and the spawned task lives until
                    // the shutdown hook cancels the token.
                    dctx.on_serve(move |_tasks, _token| {
                        let Some((routes, names)) = registry.take() else {
                            tracing::warn!(
                                "GrpcServer::on_port is installed but no gRPC service was \
                                 registered; not starting the gRPC server"
                            );
                            return;
                        };
                        r2e_core::rt::spawn(async move {
                            // Bind explicitly (instead of tonic's internal bind)
                            // so the resolved address — including an OS-assigned
                            // port for `:0` — is logged.
                            let listener = match r2e_core::rt::bind_tcp(addr.as_str()).await {
                                Ok(l) => l,
                                Err(e) => {
                                    tracing::error!(
                                        addr = %addr, error = %e,
                                        "Failed to bind gRPC listener; gRPC server NOT started"
                                    );
                                    return;
                                }
                            };
                            match listener.local_addr() {
                                Ok(local) => tracing::info!(
                                    addr = %local, services = ?names,
                                    "R2E gRPC server listening"
                                ),
                                Err(e) => tracing::warn!(
                                    error = %e,
                                    "Could not read gRPC listener local address"
                                ),
                            }
                            let incoming =
                                tonic::transport::server::TcpIncoming::from(listener);
                            if let Err(e) = tonic::transport::Server::builder()
                                .add_routes(routes)
                                .serve_with_incoming_shutdown(incoming, cancel.cancelled_owned())
                                .await
                            {
                                tracing::error!(error = %e, "gRPC server error");
                            }
                            tracing::debug!("gRPC server stopped");
                        });
                    });
                }
                GrpcTransport::Multiplexed => {
                    // Wrap the assembled HTTP router: gRPC requests (by
                    // content-type) go to the accumulated tonic routes, all
                    // others to the original router. `wrap_router` (NOT
                    // `add_layer`) puts the multiplexer OUTSIDE every HTTP
                    // layer — including other plugins' middleware and the
                    // catch-panic layer — regardless of plugin install order,
                    // so gRPC streams never cross HTTP-shaped middleware.
                    // Wraps run at build time, after every
                    // `register_grpc_service` call filled the registry.
                    // Graceful shutdown rides the HTTP server's.
                    dctx.wrap_router(Box::new(move |router| match registry.take() {
                        Some((routes, names)) => {
                            tracing::info!(
                                services = ?names,
                                "Multiplexing gRPC services onto the HTTP port \
                                 (content-type routing)"
                            );
                            let mux = MultiplexService::new(routes.prepare(), router);
                            r2e_core::http::Router::new().fallback_service(mux)
                        }
                        None => {
                            tracing::warn!(
                                "GrpcServer::multiplexed is installed but no gRPC service \
                                 was registered; serving HTTP only"
                            );
                            router
                        }
                    }));
                }
            }

            dctx.on_shutdown(move || {
                cancel_for_shutdown.cancel();
            });
        }));

        GrpcMarker
    }
}

/// Marker type provided by `GrpcServer` plugin.
///
/// This exists so the plugin can participate in the type-level provision list.
/// Users don't need to reference it directly.
#[derive(Clone)]
pub struct GrpcMarker;
