use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState;

#[derive(Controller)]
#[controller(state = AppState)]
pub struct TestGrpc;

#[grpc_routes(proto::test_service_server::TestService)]
impl TestGrpc {
    #[guard(MyGuard)]
    async fn ping(
        &self,
        _request: r2e::r2e_grpc::tonic::Request<()>,
    ) -> Result<r2e::r2e_grpc::tonic::Response<()>, r2e::r2e_grpc::tonic::Status> {
        unimplemented!()
    }
}

pub struct MyGuard;

mod proto {
    pub mod test_service_server {
        pub trait TestService {}
    }
}

fn main() {}
