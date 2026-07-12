use r2e_core::{PluginInstallContext, PreStatePlugin};

use crate::multiplex::MultiplexService;
use crate::registry::{GrpcServiceRegistry, RegisteredServices};

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
    /// `Some(extra descriptor sets)` when reflection is enabled — one field
    /// carries both the on/off state and the plugin-level extras, so they
    /// cannot desync.
    #[cfg(feature = "reflection")]
    reflection: Option<Vec<&'static [u8]>>,
}

impl GrpcServer {
    fn new(transport: GrpcTransport) -> Self {
        Self {
            transport,
            #[cfg(feature = "reflection")]
            reflection: None,
        }
    }

    /// Create a gRPC server plugin that listens on a separate port.
    pub fn on_port(addr: impl Into<String>) -> Self {
        Self::new(GrpcTransport::SeparatePort(addr.into()))
    }

    /// Create a gRPC server plugin that multiplexes with HTTP on the same port.
    pub fn multiplexed() -> Self {
        Self::new(GrpcTransport::Multiplexed)
    }

    /// Enable gRPC server reflection (v1 + v1alpha), served alongside the
    /// registered services on both transports.
    ///
    /// The reflection service answers from the encoded file descriptor sets
    /// collected at registration: each `register_grpc_service` contributes
    /// its service's set when the service declares one
    /// (`#[grpc_routes(..., descriptor = proto::FILE_DESCRIPTOR_SET)]`), and
    /// [`with_reflection_descriptor`](Self::with_reflection_descriptor) adds
    /// explicit extra sets.
    ///
    /// Reflection is only installed when at least one gRPC service is
    /// registered — with no services there is no gRPC server (and nothing
    /// true to advertise).
    ///
    /// Requires the `reflection` feature on `r2e-grpc` (`grpc-reflection` on
    /// the `r2e` facade) — without it this method does not exist, so a
    /// misconfigured build fails at compile time.
    #[cfg(feature = "reflection")]
    pub fn with_reflection(mut self) -> Self {
        self.reflection.get_or_insert_with(Vec::new);
        self
    }

    /// Enable gRPC server reflection and register an extra encoded
    /// `FileDescriptorSet` — the bytes emitted by `tonic_prost_build`'s
    /// `file_descriptor_set_path` (typically included via
    /// `tonic::include_file_descriptor_set!`).
    ///
    /// Use this for descriptor sets not carried by a registered service
    /// (e.g. when a service omits the `descriptor` argument of
    /// `#[grpc_routes]`). May be called multiple times; duplicates are
    /// stored once.
    #[cfg(feature = "reflection")]
    pub fn with_reflection_descriptor(mut self, descriptor_set: &'static [u8]) -> Self {
        crate::registry::push_unique(
            self.reflection.get_or_insert_with(Vec::new),
            descriptor_set,
        );
        self
    }
}

impl PreStatePlugin for GrpcServer {
    /// GrpcServer doesn't provide meaningful beans — it uses `GrpcMarker` as a
    /// placeholder. The real coordination happens via `GrpcServiceRegistry` in
    /// plugin_data.
    type Provided = (GrpcMarker,);
    type Deps = ();

    fn install(self, (): (), ctx: &mut PluginInstallContext<'_>) -> (GrpcMarker,) {
        let registry = GrpcServiceRegistry::new();
        let transport = self.transport;
        #[cfg(feature = "reflection")]
        let reflection = self.reflection;

        // Store the registry for register_grpc_service to find.
        ctx.store_data(registry.clone());

        match transport {
            GrpcTransport::SeparatePort(addr) => {
                // Drain the registry when the server starts and spawn the
                // tonic server next to the HTTP one. Serve hooks run before
                // the HTTP listener binds. The task observes the app
                // shutdown token as its graceful-shutdown signal, and its
                // handle is tracked so the shutdown phase awaits the gRPC
                // drain (concurrent with the HTTP drain, bounded by the
                // shutdown grace period) instead of exiting mid-drain.
                ctx.on_serve(move |serve_ctx| {
                    let Some(services) = registry.take() else {
                        tracing::warn!(
                            "GrpcServer::on_port is installed but no gRPC service was \
                             registered; not starting the gRPC server"
                        );
                        return;
                    };
                    #[cfg(feature = "reflection")]
                    let services = apply_reflection(services, &reflection);
                    let RegisteredServices { routes, names, .. } = services;
                    let cancel = serve_ctx.shutdown_token();
                    let handle = r2e_core::rt::spawn(async move {
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
                        let incoming = tonic::transport::server::TcpIncoming::from(listener);
                        if let Err(e) = tonic::transport::Server::builder()
                            .add_routes(routes)
                            .serve_with_incoming_shutdown(incoming, cancel.cancelled_owned())
                            .await
                        {
                            tracing::error!(error = %e, "gRPC server error");
                        }
                        tracing::debug!("gRPC server stopped");
                    });
                    serve_ctx.track(handle);
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
                ctx.wrap_router(move |router| match registry.take() {
                    Some(services) => {
                        #[cfg(feature = "reflection")]
                        let services = apply_reflection(services, &reflection);
                        let RegisteredServices { routes, names, .. } = services;
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
                });
            }
        }

        (GrpcMarker,)
    }
}

/// Marker type provided by `GrpcServer` plugin.
///
/// This exists so the plugin can participate in the type-level provision list.
/// Users don't need to reference it directly.
#[derive(Clone)]
pub struct GrpcMarker;

/// Fold the reflection services (v1 + v1alpha, both for client compatibility:
/// older `grpcurl` speaks v1alpha only) into the drained service set, fed by
/// the descriptor sets collected at registration plus the plugin-level extras.
/// A no-op when reflection is disabled (`None`).
///
/// Panics when a registered descriptor set fails to decode: reflection was
/// explicitly requested, so broken bytes fed to `descriptor = ...` /
/// `with_reflection_descriptor` are a build-pipeline misconfiguration that
/// must fail startup loudly, not degrade into a silently reflection-less
/// server. Both call sites run at startup, before any traffic.
#[cfg(feature = "reflection")]
fn apply_reflection(
    mut services: RegisteredServices,
    reflection: &Option<Vec<&'static [u8]>>,
) -> RegisteredServices {
    let Some(extra_descriptors) = reflection else {
        return services;
    };
    for descriptor in extra_descriptors {
        crate::registry::push_unique(&mut services.descriptors, descriptor);
    }
    if services.descriptors.is_empty() {
        tracing::warn!(
            "gRPC reflection is enabled but no file descriptor set was registered \
             (no `#[grpc_routes(..., descriptor = ...)]` service and no \
             `with_reflection_descriptor` call); reflection will only expose the \
             reflection service itself"
        );
    }

    let register = |mut builder: tonic_reflection::server::Builder<'static>| {
        for descriptor in &services.descriptors {
            builder = builder.register_encoded_file_descriptor_set(descriptor);
        }
        builder
    };
    let v1 = register(tonic_reflection::server::Builder::configure())
        .build_v1()
        .expect(
            "gRPC reflection: a registered file descriptor set failed to decode — check the \
             bytes passed to `#[grpc_routes(..., descriptor = ...)]` / \
             `with_reflection_descriptor` (must be `tonic_prost_build` \
             `file_descriptor_set_path` output)",
        );
    let v1alpha = register(tonic_reflection::server::Builder::configure())
        .build_v1alpha()
        .expect("gRPC reflection: v1alpha build failed on descriptor sets v1 accepted");
    services.routes = services.routes.add_service(v1).add_service(v1alpha);
    services.names.push("grpc.reflection.v1.ServerReflection");
    services.names.push("grpc.reflection.v1alpha.ServerReflection");
    services
}
