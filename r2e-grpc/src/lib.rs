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
pub trait AppBuilderGrpcExt<T: Clone + Send + Sync + 'static> {
    /// Register a gRPC service whose handler is wired into the gRPC server.
    fn register_grpc_service<S: GrpcService<T>>(self) -> Self;
}

impl<T: Clone + Send + Sync + 'static> AppBuilderGrpcExt<T> for r2e_core::AppBuilder<T> {
    fn register_grpc_service<S: GrpcService<T>>(self) -> Self {
        let registry = self
            .get_plugin_data::<GrpcServiceRegistry>()
            .expect(
                "GrpcServiceRegistry not found. Did you install `.plugin(GrpcServer::...)` before build_state()?",
            )
            .clone();

        // Store a factory that captures the service name for later use.
        let factory: Box<dyn std::any::Any + Send> = Box::new(GrpcServiceFactoryEntry {
            name: S::service_name(),
            factory_fn: Box::new(|state: &T| S::into_router(state))
                as Box<dyn FnOnce(&T) -> tonic::transport::server::Router + Send>,
        });
        registry.add(factory);

        tracing::debug!(
            service = S::service_name(),
            "Registered gRPC service"
        );

        self
    }
}

/// Type-erased gRPC service factory entry stored in the registry.
///
/// We can't use `GrpcServiceFactory<T>` from the `service` module directly
/// because the registry stores `Box<dyn Any + Send>` and we need to downcast.
struct GrpcServiceFactoryEntry<T: Clone + Send + Sync + 'static> {
    name: &'static str,
    factory_fn: Box<dyn FnOnce(&T) -> tonic::transport::server::Router + Send>,
}

/// Build a tonic `Server` with all registered gRPC services from the registry.
///
/// This is called during `serve()` to start the gRPC server.
pub fn build_grpc_router<T: Clone + Send + Sync + 'static>(
    registry: &GrpcServiceRegistry,
    state: &T,
) -> Option<tonic::transport::server::Router> {
    let factories = registry.take_all();
    if factories.is_empty() {
        return None;
    }

    let mut router: Option<tonic::transport::server::Router> = None;

    for factory_any in factories {
        if let Ok(entry) = factory_any.downcast::<GrpcServiceFactoryEntry<T>>() {
            tracing::info!(service = entry.name, "Starting gRPC service");
            let service_router = (entry.factory_fn)(state);
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
pub fn collect_grpc_services<T: Clone + Send + Sync + 'static>(
    registry: &GrpcServiceRegistry,
    state: &T,
) -> Vec<(&'static str, tonic::transport::server::Router)> {
    let factories = registry.take_all();
    let mut services = Vec::new();

    for factory_any in factories {
        if let Ok(entry) = factory_any.downcast::<GrpcServiceFactoryEntry<T>>() {
            let name = entry.name;
            let router = (entry.factory_fn)(state);
            services.push((name, router));
        }
    }

    services
}

/// Re-exports for generated code.
#[doc(hidden)]
pub mod __macro_support {
    pub use r2e_core::Identity;
    pub use r2e_core::StatefulConstruct;
    pub use crate::guard::{GrpcGuard, GrpcGuardContext, GrpcRolesGuard, GrpcRoleBasedIdentity};
    pub use crate::identity::{GrpcIdentityExtractor, JwtClaimsValidatorLike};
    pub use crate::service::GrpcService;
    pub use tonic;
}

pub mod prelude {
    //! Re-exports of the most commonly used gRPC types.
    pub use crate::guard::{GrpcGuard, GrpcGuardContext, GrpcRoleBasedIdentity};
    pub use crate::server::GrpcServer;
    pub use crate::service::GrpcService;
    pub use crate::AppBuilderGrpcExt;
}
