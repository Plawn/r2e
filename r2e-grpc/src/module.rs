//! Feature-module gRPC services — `#[module(grpc_services(...))]`.
//!
//! A vertical slice owns its gRPC service exactly like its HTTP controllers:
//! the service is dependency-checked **module-locally** at `register_module`
//! (against `ModuleScope<M>` — provided ∪ brought-plugin beans ∪ imports) and
//! registered by `build_state()` from the retained
//! [`BeanContext`](r2e_core::beans::BeanContext), so it may inject the
//! module's **private** providers, which the app-level
//! [`register_grpc_service`](crate::AppBuilderGrpcExt::register_grpc_service)
//! cannot (its deps must be in the application state).
//!
//! r2e-grpc depends on r2e-core, never the reverse, so the hook is generic:
//! r2e-core declares [`ModuleEndpointSet`] (type-level deps) and
//! [`ModuleEndpoints`] (value-level registration) and this module implements
//! both for [`ModuleGrpcServices`]. The wrapper exists because orphan rules
//! forbid implementing a foreign trait for a bare tuple of type parameters.
//!
//! ```ignore
//! #[module(
//!     providers(GreetingRepo, GreetingService),  // both private
//!     grpc_services(GreeterService),             // injects them
//!     controllers(GreetingController),
//! )]
//! pub struct GreetingModule;
//!
//! AppBuilder::new()
//!     .plugin(GrpcServer::on_port("0.0.0.0:50051"))  // required by the module
//!     .register_module::<GreetingModule>()
//!     .build_state()
//!     .await
//! ```

use std::marker::PhantomData;

use r2e_core::beans::BeanError;
use r2e_core::di::module::{ModuleEndpointSet, ModuleEndpoints};
use r2e_core::type_list::{TAppend, TNil};
use r2e_core::{AppBuilder, EndpointDeps};

use crate::service::GrpcService;
use crate::RegisterServiceError;

/// A feature module's gRPC service set: the value of
/// [`FeatureModule::Endpoints`](r2e_core::FeatureModule::Endpoints) that
/// `#[module(grpc_services(A, B))]` generates as
/// `ModuleGrpcServices<(A, B)>`.
///
/// Marker only — never constructed. Implemented for tuples of arity 0..=8.
pub struct ModuleGrpcServices<S>(PhantomData<fn() -> S>);

/// Register one module-owned gRPC service into the typed builder, **without**
/// the application-state dependency check (it was already checked against the
/// module scope at `register_module`), mapping the failure modes onto the boot
/// error channel.
fn register_module_service<T, S>(builder: AppBuilder<T>) -> Result<AppBuilder<T>, BeanError>
where
    T: Clone + Send + Sync + 'static,
    S: GrpcService,
{
    match crate::register_service_unchecked::<T, S>(&builder) {
        Ok(()) => Ok(builder),
        Err(RegisterServiceError::Config(source)) => Err(BeanError::EndpointConfig {
            endpoint: std::any::type_name::<S>(),
            source,
        }),
        Err(RegisterServiceError::MissingPlugin) => Err(BeanError::MissingTransportPlugin {
            endpoint: std::any::type_name::<S>(),
            plugin: "GrpcServer",
        }),
    }
}

impl ModuleEndpointSet for ModuleGrpcServices<()> {
    type Deps = TNil;
}

impl<T: Clone + Send + Sync + 'static> ModuleEndpoints<T> for ModuleGrpcServices<()> {
    fn register_all(builder: AppBuilder<T>) -> Result<AppBuilder<T>, BeanError> {
        Ok(builder)
    }
}

macro_rules! impl_module_grpc_services {
    ($S0:ident) => {
        impl<$S0: EndpointDeps> ModuleEndpointSet for ModuleGrpcServices<($S0,)>
        where
            $S0::Deps: TAppend<TNil>,
        {
            type Deps = <$S0::Deps as TAppend<TNil>>::Output;
        }

        impl<T, $S0> ModuleEndpoints<T> for ModuleGrpcServices<($S0,)>
        where
            T: Clone + Send + Sync + 'static,
            $S0: GrpcService,
        {
            fn register_all(builder: AppBuilder<T>) -> Result<AppBuilder<T>, BeanError> {
                register_module_service::<T, $S0>(builder)
            }
        }
    };
    ($S0:ident, $($Ss:ident),+) => {
        impl<$S0: EndpointDeps, $($Ss: EndpointDeps),+> ModuleEndpointSet
            for ModuleGrpcServices<($S0, $($Ss),+)>
        where
            ModuleGrpcServices<($($Ss,)+)>: ModuleEndpointSet,
            $S0::Deps: TAppend<<ModuleGrpcServices<($($Ss,)+)> as ModuleEndpointSet>::Deps>,
        {
            type Deps = <$S0::Deps as TAppend<
                <ModuleGrpcServices<($($Ss,)+)> as ModuleEndpointSet>::Deps,
            >>::Output;
        }

        impl<T, $S0, $($Ss),+> ModuleEndpoints<T> for ModuleGrpcServices<($S0, $($Ss),+)>
        where
            T: Clone + Send + Sync + 'static,
            $S0: GrpcService,
            $($Ss: GrpcService,)+
        {
            fn register_all(builder: AppBuilder<T>) -> Result<AppBuilder<T>, BeanError> {
                let builder = register_module_service::<T, $S0>(builder)?;
                $(let builder = register_module_service::<T, $Ss>(builder)?;)+
                Ok(builder)
            }
        }

        impl_module_grpc_services!($($Ss),+);
    };
}

impl_module_grpc_services!(S0, S1, S2, S3, S4, S5, S6, S7);
