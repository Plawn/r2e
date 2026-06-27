use std::future::Future;

use r2e::prelude::*;
use r2e::sse::{SseBroadcaster, SseSubscription};
use r2e::ws::WsStream;
use r2e::{Guard, GuardContext, Identity, PathParam};
use serde::Deserialize;

#[derive(Clone)]
pub struct AppState;

#[derive(Clone, Copy, Deserialize)]
pub struct ProjectId(u64);

#[derive(Clone, Copy, Deserialize)]
pub struct StreamId(u64);

pub struct StreamGuard;

impl StreamGuard {
    pub const fn viewer(_project: PathParam<ProjectId>, _stream: PathParam<StreamId>) -> Self {
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

#[derive(Controller)]
#[controller(path = "/projects/{pid}", state = AppState)]
pub struct RealtimeController;

#[routes]
impl RealtimeController {
    #[sse("/streams/{sid}/events")]
    #[guard(StreamGuard::viewer(path::pid, path::sid))]
    async fn events(&self, Path((_pid, _sid)): Path<(ProjectId, StreamId)>) -> SseSubscription {
        SseBroadcaster::new(1).subscribe()
    }

    #[ws("/streams/{sid}/socket")]
    #[guard(StreamGuard::viewer(path::pid, path::sid))]
    async fn socket(&self, Path((_pid, _sid)): Path<(ProjectId, StreamId)>, _ws: WsStream) {}
}

fn main() {}
