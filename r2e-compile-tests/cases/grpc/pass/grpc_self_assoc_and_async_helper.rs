//! `#[grpc_routes]` methods run on the request façade, where `Self` is the
//! façade, so `Self::<Assoc>` paths are rewritten to the declared associated
//! type — structurally: a generic declared type used as a constructor path
//! (`Self::WatchStream::new(rx)`) becomes the expression-form path
//! `ReceiverStream::<..>::new(rx)`, and a type annotation
//! (`let s: Self::WatchStream`) the declared type.
//! The rewrite reaches turbofish arguments (`::wrap::<Self::WatchStream>`),
//! macro arguments (`dbg!`, `matches!`, including a guarded pattern that
//! only the token-level fallback handles), struct literals and patterns
//! (tuple-struct, struct, unit), and stops at nested items: a local `impl`
//! with its own `type WatchStream` keeps its own `Self::WatchStream`.
//! An `async` `#[request_helper]` with `&self` is a façade helper, not an rpc.

use r2e::prelude::*;
use r2e::r2e_grpc::AppBuilderGrpcExt;
use r2e::rt::stream::wrappers::ReceiverStream;
use std::pin::Pin;
use std::task::{Context, Poll};

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
        let stream: Self::WatchStream = dbg!(Self::WatchStream::new(rx));

        // A nested impl introduces its own `Self`: not rewritten.
        trait Local {
            type WatchStream;
            fn get() -> Self::WatchStream;
        }
        struct Here;
        impl Local for Here {
            type WatchStream = u8;
            fn get() -> Self::WatchStream {
                7
            }
        }
        let _: u8 = <Here as Local>::get();

        Ok(r2e::r2e_grpc::tonic::Response::new(stream))
    }

    #[request_helper]
    async fn greeting(&self, msg: String) -> String {
        format!("{msg} ({} headers)", self.headers.len())
    }
}

/// A declared stream type that is an enum, so `Self::WatchStream::…` can be
/// a constructor, a struct literal and a pattern.
#[derive(Debug)]
pub enum Replies {
    Live(ReceiverStream<Result<ping::PingReply, Status>>),
    Done { reason: &'static str },
    Idle,
}

impl Replies {
    fn wrap<T: Into<Replies>>(value: T) -> Replies {
        value.into()
    }
}

impl r2e::rt::stream::Stream for Replies {
    type Item = Result<ping::PingReply, Status>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.get_mut() {
            Replies::Live(s) => Pin::new(s).poll_next(cx),
            Replies::Done { .. } | Replies::Idle => Poll::Ready(None),
        }
    }
}

#[controller]
pub struct PatternWatchService {}

#[grpc_routes(ping::watch_server::Watch)]
impl PatternWatchService {
    type WatchStream = Replies;

    async fn watch(
        &self,
        request: r2e::r2e_grpc::tonic::Request<ping::PingRequest>,
    ) -> Result<r2e::r2e_grpc::tonic::Response<Self::WatchStream>, Status> {
        let (_tx, rx) = r2e::rt::sync::mpsc::channel(1);
        // Tuple-variant constructor, turbofish argument.
        let live = Self::WatchStream::Live(ReceiverStream::new(rx));
        let live = Self::WatchStream::wrap::<Self::WatchStream>(live);
        // Struct literal and unit value.
        let done = Self::WatchStream::Done { reason: "empty" };
        let idle = Self::WatchStream::Idle;
        // Patterns: struct, tuple-struct, unit.
        if let Self::WatchStream::Done { reason } = &done {
            let _ = reason;
        }
        match &idle {
            Self::WatchStream::Live(_) => {}
            Self::WatchStream::Done { .. } => {}
            Self::WatchStream::Idle => {}
        }
        // Macro arguments: an expression list, and a guarded pattern that
        // only the token-level fallback rewrites.
        assert!(matches!(live, Self::WatchStream::Live(_)));
        assert!(!matches!(done, Self::WatchStream::Done { .. } if request.get_ref().msg.is_empty()));
        Ok(r2e::r2e_grpc::tonic::Response::new(live))
    }
}

fn main() {
    let _ = async {
        AppBuilder::new()
            .build_state()
            .await
            .register_grpc_service::<WatchService>()
            .register_grpc_service::<PatternWatchService>()
    };
}
