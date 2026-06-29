use std::future::Future;

use r2e::prelude::*;
use r2e::sse::{SseBroadcaster, SseSubscription};
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

#[controller(path = "/events", state = AppState)]
pub struct EventsController;

#[routes]
impl EventsController {
    #[sse("/{id}")]
    #[guard(StreamGuard::viewer(path::missing))]
    async fn events(&self, Path(_id): Path<u64>) -> SseSubscription {
        SseBroadcaster::new(1).subscribe()
    }
}

fn main() {}
