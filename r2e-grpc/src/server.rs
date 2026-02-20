use r2e_core::type_list::TNil;
use r2e_core::{DeferredAction, PluginInstallContext, PreStatePlugin};
use tokio_util::sync::CancellationToken;

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
/// Install as a `PreStatePlugin` before `build_state()`.
///
/// # Example
///
/// ```ignore
/// use r2e_grpc::GrpcServer;
///
/// AppBuilder::new()
///     .plugin(GrpcServer::on_port("0.0.0.0:50051"))
///     // or: .plugin(GrpcServer::multiplexed())
///     .build_state::<Services, _, _>()
///     .await
///     .register_grpc_service::<UserGrpcService>()
///     .serve("0.0.0.0:3000")
/// ```
pub struct GrpcServer {
    transport: GrpcTransport,
    #[allow(dead_code)]
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

    /// Enable gRPC server reflection (requires the `reflection` feature).
    pub fn with_reflection(mut self) -> Self {
        self.reflection = true;
        self
    }
}

impl PreStatePlugin for GrpcServer {
    /// GrpcServer doesn't provide beans — it uses `GrpcMarker` as a placeholder.
    /// The real coordination happens via `GrpcServiceRegistry` in plugin_data.
    type Provided = GrpcMarker;
    type Required = TNil;

    fn install(self, ctx: &mut PluginInstallContext) -> GrpcMarker {
        let registry = GrpcServiceRegistry::new();
        let transport = self.transport.clone();
        let cancel = CancellationToken::new();
        let cancel_for_shutdown = cancel.clone();

        ctx.add_deferred(DeferredAction::new("GrpcServer", move |dctx| {
            // Store the registry for register_grpc_service to find.
            dctx.store_data(registry.clone());
            // Store the transport config for the serve hook to use.
            dctx.store_data(GrpcTransportConfig(transport));

            dctx.on_serve(move |_tasks, _token| {
                // The actual server startup is handled by the serve() extension
                // or by the builder, since we need access to the collected services.
                // The GrpcServiceRegistry is read during serve().
            });

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

/// Wrapper to store the transport config in plugin_data.
pub(crate) struct GrpcTransportConfig(pub(crate) GrpcTransport);
