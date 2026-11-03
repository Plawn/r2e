//! `#[grpc_routes]` methods run on the request façade, where `Self` is the
//! façade, so `Self::<Assoc>` paths are rewritten to the declared associated
//! type — structurally: a generic declared type used as a constructor path
//! (`Self::WatchStream::new(rx)`) becomes the qualified `<Declared>::new(rx)`,
//! and a type annotation (`let s: Self::WatchStream`) the declared type.
//! An `async` `#[request_helper]` with `&self` is a façade helper, not an rpc.

use r2e::prelude::*;
use r2e::r2e_grpc::AppBuilderGrpcExt;
use r2e::rt::stream::wrappers::ReceiverStream;

use r2e_compile_tests::proto::ping;

type Status = r2e::r2e_grpc::tonic::Status;

#[controller]
pub struct WatchService {
    #[inject(request)]
    headers: HeaderMap,
}

#[grpc_routes(ping::watch_server::Watch)]
impl WatchService {
    type WatchStream = ReceiverStream<Result<ping::PingReply, Status>>;

    async fn watch(
        &self,
        request: r2e::r2e_grpc::tonic::Request<ping::PingRequest>,
    ) -> Result<r2e::r2e_grpc::tonic::Response<Self::WatchStream>, Status> {
        let msg = self.greeting(request.into_inner().msg).await;
        let (tx, rx) = r2e::rt::sync::mpsc::channel(1);
        let _ = tx.try_send(Ok(ping::PingReply { msg }));
        let stream: Self::WatchStream = Self::WatchStream::new(rx);
        Ok(r2e::r2e_grpc::tonic::Response::new(stream))
    }

    #[request_helper]
    async fn greeting(&self, msg: String) -> String {
        format!("{msg} ({} headers)", self.headers.len())
    }
}

fn main() {
    let _ = async {
        AppBuilder::new()
            .build_state()
            .await
            .register_grpc_service::<WatchService>()
    };
}
