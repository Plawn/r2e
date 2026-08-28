use r2e_core::plugin::{PluginBuildContext, PluginBuildError};
use r2e_core::Plugin;

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
/// Install as a `Plugin` before `build_state()`. The plugin stores a
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
    /// `Some(cors)` when the multiplexed transport should serve grpc-web
    /// through a `tonic-web` arm, with that CORS policy in front of it.
    #[cfg(feature = "web")]
    grpc_web: Option<tower_http::cors::CorsLayer>,
}

impl GrpcServer {
    fn new(transport: GrpcTransport) -> Self {
        Self {
            transport,
            #[cfg(feature = "reflection")]
            reflection: None,
            #[cfg(feature = "web")]
            grpc_web: None,
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

    /// Serve grpc-web (`application/grpc-web*`, binary and `-text`, over
    /// HTTP/1.1 and HTTP/2) on the multiplexed port, translated by
    /// `tonic-web`, with [`web::default_cors`](crate::multiplex::web::default_cors)
    /// in front of it (any origin, gRPC status trailers exposed) so browser
    /// clients work without any other CORS setup. Use
    /// [`with_grpc_web_cors`](Self::with_grpc_web_cors) to bring your own
    /// policy.
    ///
    /// Only meaningful with [`multiplexed`](Self::multiplexed); on the
    /// separate-port transport it is ignored with a boot warning. Requires
    /// the `web` feature on `r2e-grpc` (`grpc-web` on the `r2e` facade).
    #[cfg(feature = "web")]
    pub fn with_grpc_web(self) -> Self {
        self.with_grpc_web_cors(crate::multiplex::web::default_cors())
    }

    /// Like [`with_grpc_web`](Self::with_grpc_web), with an explicit CORS
    /// policy for the grpc-web arm. It also answers grpc-web preflights
    /// (`OPTIONS` naming `x-grpc-web`), so remember to allow `POST`, the
    /// `content-type` / `x-grpc-web` / `x-user-agent` / `grpc-timeout`
    /// request headers, and to expose `grpc-status` / `grpc-message` /
    /// `grpc-status-details-bin`.
    #[cfg(feature = "web")]
    pub fn with_grpc_web_cors(mut self, cors: tower_http::cors::CorsLayer) -> Self {
        self.grpc_web = Some(cors);
        self
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
        crate::registry::push_unique(self.reflection.get_or_insert_with(Vec::new), descriptor_set);
        self
    }
}

impl Plugin for GrpcServer {
    /// GrpcServer doesn't provide meaningful beans — it uses `GrpcMarker` as a
    /// placeholder. The real coordination happens via `GrpcServiceRegistry` in
    /// plugin_data.
    type Provided = (GrpcMarker,);
    type Deps = ();
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        _deps: Self::Deps,
        _config: Option<Self::Config>,
        ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        let registry = GrpcServiceRegistry::new();
        let transport = self.transport;
        #[cfg(feature = "reflection")]
        let reflection = self.reflection;
        #[cfg(feature = "web")]
        let grpc_web = self.grpc_web;

        // Store the registry for register_grpc_service to find.
        ctx.store_data(registry.clone());

        match transport {
            GrpcTransport::SeparatePort(addr) => {
                #[cfg(feature = "web")]
                if grpc_web.is_some() {
                    tracing::warn!(
                        "GrpcServer::with_grpc_web is only honoured by the multiplexed \
                         transport; the separate-port gRPC server serves native gRPC only"
                    );
                }
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
                    // Phase 2 will move this crate onto `CancelToken`; until then
                    // the seam hands out the raw tokio-util token, which tonic's
                    // `serve_with_incoming_shutdown` needs for `cancelled_owned()`.
                    let cancel = serve_ctx.shutdown_token().into_inner();
                    serve_ctx.track_named("grpc server", async move {
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
                        #[cfg(feature = "web")]
                        if let Some(cors) = grpc_web {
                            tracing::info!(
                                "grpc-web enabled on the multiplexed port (tonic-web arm, \
                                 HTTP/1.1 + HTTP/2, binary and -text)"
                            );
                            let web =
                                crate::multiplex::web::grpc_web_arm(routes.clone().prepare(), cors);
                            let mux =
                                MultiplexService::new(routes.prepare(), router).with_grpc_web(web);
                            return r2e_core::http::Router::new().fallback_service(mux);
                        }
                        // Said once at boot so it is not a surprise at the
                        // first browser call: without a grpc-web arm,
                        // `application/grpc-web*` requests are answered
                        // with 415, not proxied.
                        tracing::warn!(
                            "{} — `application/grpc-web*` requests are answered with \
                             415 Unsupported Media Type. Enable it with \
                             `GrpcServer::multiplexed().with_grpc_web()` (feature \
                             `grpc-web`), use a native gRPC client, or put a grpc-web \
                             proxy (Envoy, …) in front.",
                            crate::multiplex::GRPC_WEB_UNSUPPORTED
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

        Ok((GrpcMarker,))
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
    services
        .names
        .push("grpc.reflection.v1alpha.ServerReflection");
    services
}
