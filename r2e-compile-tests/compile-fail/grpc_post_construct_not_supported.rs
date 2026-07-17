use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState;

#[controller]
pub struct TestGrpc;

#[grpc_routes(proto::test_service_server::TestService)]
impl TestGrpc {
    // Async `&self`: without the disallow check this would land in the tonic
    // trait impl and fail with a confusing E0407 "not a member of trait".
    #[post_construct]
    async fn warm_up(&self) {}

    async fn ping(
        &self,
        _request: r2e::r2e_grpc::tonic::Request<()>,
    ) -> Result<r2e::r2e_grpc::tonic::Response<()>, r2e::r2e_grpc::tonic::Status> {
        unimplemented!()
    }
}

mod proto {
    pub mod test_service_server {
        pub trait TestService {}
    }
}

fn main() {}
