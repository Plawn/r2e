use std::future::Future;

use r2e::prelude::*;
use r2e::ws::WsStream;
use r2e::{Guard, GuardContext, Identity, PathParam};

#[derive(Clone)]
pub struct AppState;

pub struct StreamGuard;

impl StreamGuard {
    pub const fn viewer(_param: PathParam<u64>) -> Self {
        Self
    }
}

impl<S: Send + Sync, I: Identity> Guard<S, I> for StreamGuard {
    fn check(
        &self,
        _state: &S,
        _ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async { Ok(()) }
    }
}

#[controller(path = "/socket")]
pub struct SocketController;

#[routes]
impl SocketController {
    #[ws("/{id}")]
    #[guard(StreamGuard::viewer(path::missing))]
    async fn socket(&self, Path(_id): Path<u64>, _ws: WsStream) {}
}

fn main() {}
