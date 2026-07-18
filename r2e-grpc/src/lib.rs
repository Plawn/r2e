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
//!     .build_state()
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

use r2e_core::type_list::AllSatisfied;
use r2e_core::EndpointDeps;

pub use guard::{GrpcGuard, GrpcGuardContext, GrpcRolesGuard, GrpcRoleBasedIdentity};
pub use identity::{
    extract_bearer_token, extract_jwt_claims_from_metadata, GrpcIdentityExtractor,
    JwtClaimsValidatorLike,
};
pub use multiplex::MultiplexService;
pub use registry::{GrpcServiceRegistry, RegisteredServices};
pub use server::{GrpcMarker, GrpcServer, GrpcTransport};
pub use service::GrpcService;

// Re-export tonic for use by generated code.
pub use tonic;
pub use prost;

/// Extension trait for `AppBuilder` to register gRPC services.
///
/// This is the gRPC analog of `register_controller` for HTTP — including the
/// compile-time dependency check: the service's [`EndpointDeps`] (its core's
/// `#[inject]` fields plus every `#[intercept(...)]` spec's deps, emitted by
/// `#[grpc_routes]`) are checked against the application state via
/// `AllSatisfied`, so a missing bean is a compile error at this call site.
///
/// `T` and `DepIdx` are inference-only witnesses (the same pattern as
/// [`RegisterController`](r2e_core::RegisterController)): call sites write
/// `.register_grpc_service::<UserGrpcService>()` and never name them.
///
/// # Example
///
/// ```ignore
/// use r2e_grpc::AppBuilderGrpcExt;
///
/// AppBuilder::new()
///     .plugin(GrpcServer::on_port("0.0.0.0:50051"))
///     .build_state()
///     .await
///     .register_grpc_service::<UserGrpcService>()
///     .register_grpc_service::<OrderGrpcService>()
///     .serve("0.0.0.0:3000")
/// ```
pub trait AppBuilderGrpcExt<T, DepIdx>: Sized
where
    T: Clone + Send + Sync + 'static,
{
    /// Register a gRPC service whose handler is wired into the gRPC server.
    ///
    /// The service is built immediately from the retained bean graph
    /// ([`AppBuilder::bean_context`](r2e_core::AppBuilder::bean_context)).
    fn register_grpc_service<S>(self) -> Self
    where
        S: GrpcService + EndpointDeps,
        S::Deps: AllSatisfied<T, DepIdx>;
}

impl<T, DepIdx> AppBuilderGrpcExt<T, DepIdx> for r2e_core::AppBuilder<T>
where
    T: Clone + Send + Sync + 'static,
{
    fn register_grpc_service<S>(self) -> Self
    where
        S: GrpcService + EndpointDeps,
        S::Deps: AllSatisfied<T, DepIdx>,
    {
        let registry = self
            .get_plugin_data::<GrpcServiceRegistry>()
            .expect(
                "GrpcServiceRegistry not found. Did you install `.plugin(GrpcServer::...)` before build_state()?",
            )
            .clone();

        registry.add_service(S::service_name(), S::file_descriptor_set(), |routes| {
            S::add_to_routes(routes, self.bean_context())
        });

        tracing::debug!(
            service = S::service_name(),
            "Registered gRPC service"
        );

        self
    }
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
}

pub mod prelude {
    //! Re-exports of the most commonly used gRPC types.
    pub use crate::guard::{GrpcGuard, GrpcGuardContext, GrpcRoleBasedIdentity};
    pub use crate::server::GrpcServer;
    pub use crate::service::GrpcService;
    pub use crate::AppBuilderGrpcExt;
}
