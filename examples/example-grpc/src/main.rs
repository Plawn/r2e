use r2e::prelude::*;
use r2e::r2e_grpc::{GrpcServer, AppBuilderGrpcExt};

pub mod proto {
    tonic::include_proto!("greeter");
}

use proto::{HelloReply, HelloRequest};

// ── State ──────────────────────────────────────────────────────────────

#[derive(Clone, BeanState)]
pub struct Services {
    pub greeting_prefix: GreetingPrefix,
}

#[derive(Clone)]
pub struct GreetingPrefix(pub String);

// ── gRPC Service ───────────────────────────────────────────────────────

#[derive(Controller)]
#[controller(state = Services)]
pub struct GreeterService {
    #[inject]
    greeting_prefix: GreetingPrefix,
}

#[grpc_routes(proto::greeter_server::Greeter)]
impl GreeterService {
    async fn say_hello(
        &self,
        request: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        let name = &request.get_ref().name;
        let reply = HelloReply {
            message: format!("{} {}!", self.greeting_prefix.0, name),
        };
        Ok(tonic::Response::new(reply))
    }

    async fn say_hello_admin(
        &self,
        request: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        let name = &request.get_ref().name;
        let reply = HelloReply {
            message: format!("[ADMIN] {} {}!", self.greeting_prefix.0, name),
        };
        Ok(tonic::Response::new(reply))
    }
}

// ── HTTP Controller (to show multiplexing) ─────────────────────────────

#[derive(Controller)]
#[controller(path = "/api", state = Services)]
pub struct HealthController;

#[routes]
impl HealthController {
    #[get("/health")]
    async fn health(&self) -> &'static str {
        "OK"
    }
}

// ── Main ───────────────────────────────────────────────────────────────

#[r2e::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let prefix = GreetingPrefix("Hello".to_string());

    let app = AppBuilder::new()
        .plugin(GrpcServer::on_port("0.0.0.0:50051"))
        .provide(prefix)
        .build_state::<Services, _, _>()
        .await
        .register_grpc_service::<GreeterService>()
        .register_controller::<HealthController>();

    tracing::info!("HTTP on :3000, gRPC on :50051");
    app.serve("0.0.0.0:3000").await
}
