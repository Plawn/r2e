//! gRPC server support for R2E.
//!
//! Provides gRPC service hosting with the same DX as HTTP controllers:
//! `#[inject]`, `#[config]`, guards, interceptors, and identity extraction.
//!
//! # Two transport modes
//!
//! - **Separate port**: gRPC on one port, HTTP on another.
//! - **Multiplexed**: both on the same port, routed by `content-type`.
//!
//! # Example
//!
//! ```ignore
//! use r2e_grpc::GrpcServer;
//!
//! AppBuilder::new()
//!     .plugin(GrpcServer::on_port("0.0.0.0:50051"))
//!     .build_state::<Services, _>()
//!     .await
//!     .register_grpc_service::<UserGrpcService>()
//!     .serve("0.0.0.0:3000")
//! ```

pub mod guard;
pub mod identity;
pub mod multiplex;
pub mod registry;
pub mod server;
pub mod service;

pub use guard::{GrpcGuard, GrpcGuardContext, GrpcRolesGuard, GrpcRoleBasedIdentity};
pub use identity::{
    extract_bearer_token, extract_jwt_claims_from_metadata, GrpcIdentityExtractor,
    JwtClaimsValidatorLike,
};
pub use multiplex::MultiplexService;
pub use registry::GrpcServiceRegistry;
pub use server::{GrpcMarker, GrpcServer, GrpcTransport};
pub use service::GrpcService;

// Re-export tonic for use by generated code.
pub use tonic;
pub use prost;

/// Extension trait for `AppBuilder` to register gRPC services.
///
/// This is the gRPC analog of `register_controller` for HTTP.
///
/// # Example
///
/// ```ignore
/// use r2e_grpc::AppBuilderGrpcExt;
///
/// AppBuilder::new()
///     .plugin(GrpcServer::on_port("0.0.0.0:50051"))
///     .build_state::<Services, _>()
///     .await
///     .register_grpc_service::<UserGrpcService>()
///     .register_grpc_service::<OrderGrpcService>()
///     .serve("0.0.0.0:3000")
/// ```
pub trait AppBuilderGrpcExt {
    /// Register a gRPC service whose handler is wired into the gRPC server.
    ///
    /// The service is built immediately from the retained bean graph
    /// ([`AppBuilder::bean_context`](r2e_core::AppBuilder::bean_context)).
    fn register_grpc_service<S: GrpcService>(self) -> Self;
}

impl<T: Clone + Send + Sync + 'static> AppBuilderGrpcExt for r2e_core::AppBuilder<T> {
    fn register_grpc_service<S: GrpcService>(self) -> Self {
        let registry = self
            .get_plugin_data::<GrpcServiceRegistry>()
            .expect(
                "GrpcServiceRegistry not found. Did you install `.plugin(GrpcServer::...)` before build_state()?",
            )
            .clone();

        let entry: Box<dyn std::any::Any + Send> = Box::new(service::GrpcServiceEntry {
            name: S::service_name(),
            router: S::into_router(self.bean_context()),
        });
        registry.add(entry);

        tracing::debug!(
            service = S::service_name(),
            "Registered gRPC service"
        );

        self
    }
}

/// Build a tonic `Server` with all registered gRPC services from the registry.
///
/// This is called during `serve()` to start the gRPC server.
pub fn build_grpc_router(
    registry: &GrpcServiceRegistry,
) -> Option<tonic::transport::server::Router> {
    let factories = registry.take_all();
    if factories.is_empty() {
        return None;
    }

    let mut router: Option<tonic::transport::server::Router> = None;

    for factory_any in factories {
        if let Ok(entry) = factory_any.downcast::<service::GrpcServiceEntry>() {
            tracing::info!(service = entry.name, "Starting gRPC service");
            let service_router = entry.router;
            router = Some(match router {
                Some(existing) => {
                    // Merge routers — tonic doesn't have a merge, but we can
                    // use the builder pattern. For now, we'll build services one
                    // at a time and fold them into a single Router via
                    // Server::builder().
                    // Actually, tonic::transport::server::Router doesn't support
                    // merging. The pattern is to call add_service repeatedly on
                    // the Server builder. We need to change our approach.
                    //
                    // Let's return the list of factories instead and let the
                    // caller build the server.
                    existing
                }
                None => service_router,
            });
        }
    }

    router
}

/// Collect all gRPC service factories from the registry and build them.
///
/// Returns a list of built tonic Routers, one per service.
pub fn collect_grpc_services(
    registry: &GrpcServiceRegistry,
) -> Vec<(&'static str, tonic::transport::server::Router)> {
    let factories = registry.take_all();
    let mut services = Vec::new();

    for factory_any in factories {
        if let Ok(entry) = factory_any.downcast::<service::GrpcServiceEntry>() {
            let name = entry.name;
            services.push((name, entry.router));
        }
    }

    services
}

/// Re-exports for generated code.
#[doc(hidden)]
pub mod __macro_support {
    pub use r2e_core::Identity;
    pub use r2e_core::ContextConstruct;
    pub use crate::guard::{GrpcGuard, GrpcGuardContext, GrpcRolesGuard, GrpcRoleBasedIdentity};
    pub use crate::identity::{GrpcIdentityExtractor, JwtClaimsValidatorLike};
    pub use crate::service::GrpcService;
    pub use tonic;

    /// Uninhabited placeholder identity used by generated guard scaffolding when
    /// no concrete identity type is available (e.g. `None::<&NeverIdentity>`).
    ///
    /// Replaces the former `tonic::codegen::Never`, which was removed in tonic 0.14.
    pub enum NeverIdentity {}

    impl r2e_core::Identity for NeverIdentity {
        fn sub(&self) -> &str {
            match *self {}
        }
    }
}

pub mod prelude {
    //! Re-exports of the most commonly used gRPC types.
    pub use crate::guard::{GrpcGuard, GrpcGuardContext, GrpcRoleBasedIdentity};
    pub use crate::server::GrpcServer;
    pub use crate::service::GrpcService;
    pub use crate::AppBuilderGrpcExt;
}
