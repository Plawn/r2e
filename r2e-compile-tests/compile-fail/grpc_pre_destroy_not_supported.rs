use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState;

#[controller]
pub struct TestGrpc;

#[grpc_routes(proto::test_service_server::TestService)]
impl TestGrpc {
    async fn ping(
        &self,
        _request: r2e::r2e_grpc::tonic::Request<()>,
    ) -> Result<r2e::r2e_grpc::tonic::Response<()>, r2e::r2e_grpc::tonic::Status> {
        unimplemented!()
    }

    // Sync helper: without the disallow check this would be re-emitted
    // verbatim into `other_methods` and silently never run.
    #[pre_destroy]
    fn close(&self) {}
}

mod proto {
    pub mod test_service_server {
        pub trait TestService {}
    }
}

fn main() {}
