//! Scaffolding for `llm/modules.md`.

use r2e::prelude::*;

pub use std::sync::Arc;

pub use sqlx::SqlitePool;

/// Plugins the module snippets install or require (none is in the prelude).
pub use r2e::r2e_executor::Executor;
pub use r2e::r2e_grpc::{GrpcServer, GrpcService};
pub use r2e::{BeanContext, EndpointDeps};

/// The slice's own service — the module's `providers(..)` / `exports(..)`.
#[derive(Clone)]
pub struct UserService;

#[bean]
impl UserService {
    pub fn new() -> Self {
        Self
    }

    pub async fn list(&self) -> Vec<String> {
        Vec::new()
    }
}

/// The slice's controller.
#[controller(path = "/users")]
pub struct UserController {
    #[inject]
    user_service: UserService,
}

#[routes]
impl UserController {
    #[get("/")]
    async fn list(&self) -> Json<Vec<String>> {
        Json(self.user_service.list().await)
    }
}

/// The event the module's consumer controller reads.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct UserCreated {
    pub id: i64,
}

/// A second controller of the slice — a `#[consumer]`, not a route.
#[controller]
pub struct UserEventConsumer {
    #[inject]
    event_bus: LocalEventBus,
}

#[routes]
impl UserEventConsumer {
    #[consumer(bus = "event_bus")]
    async fn on_created(&self, _event: Arc<UserCreated>) {}
}

/// A gRPC service of the slice. `#[grpc_routes]` writes these two impls from a
/// tonic-generated server trait; the doctest scaffold spells them out so the
/// module snippets need no generated proto module.
pub struct UserGrpcService;

impl EndpointDeps for UserGrpcService {
    type Deps = r2e::type_list::TNil;
}

impl GrpcService for UserGrpcService {
    fn service_name() -> &'static str {
        "demo.Users"
    }

    fn add_to_routes(
        routes: r2e::r2e_grpc::tonic::service::Routes,
        _ctx: &Arc<BeanContext>,
    ) -> r2e::r2e_grpc::tonic::service::Routes {
        routes
    }
}

/// The module's PRIVATE provider in the `grpc_services` snippet.
#[derive(Clone)]
pub struct GreetingRepo;

#[bean]
impl GreetingRepo {
    pub fn new() -> Self {
        Self
    }
}

/// The gRPC service that injects the private provider.
pub struct GreeterService;

impl EndpointDeps for GreeterService {
    type Deps = r2e::type_list::TCons<GreetingRepo, r2e::type_list::TNil>;
}

impl GrpcService for GreeterService {
    fn service_name() -> &'static str {
        "demo.Greeter"
    }

    fn add_to_routes(
        routes: r2e::r2e_grpc::tonic::service::Routes,
        _ctx: &Arc<BeanContext>,
    ) -> r2e::r2e_grpc::tonic::service::Routes {
        routes
    }
}

/// Sibling slices, for the `imports(module(..))` and aggregate snippets.
#[derive(Clone)]
pub struct BillingService;

#[bean]
impl BillingService {
    pub fn new() -> Self {
        Self
    }
}

#[module(providers(BillingService), exports(BillingService))]
pub struct BillingModule;

#[derive(Clone)]
pub struct ReportService;

#[bean]
impl ReportService {
    pub fn new() -> Self {
        Self
    }
}

#[module(providers(ReportService), exports(ReportService))]
pub struct ReportModule;

/// The simple form of the slice the aggregate snippet lists (the doc's own
/// block redefines `UserModule` with the full set of keys).
#[module(
    providers(UserService),
    controllers(UserController),
    exports(UserService)
)]
pub struct UserModule;
