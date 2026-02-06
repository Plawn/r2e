use std::convert::Infallible;

use r2e::prelude::*;
use r2e::http::response::SseEvent;
use r2e::sse::SseBroadcaster;

use crate::state::Services;

#[derive(Controller)]
#[controller(path = "/sse", state = Services)]
pub struct SseController {
    #[inject]
    sse_broadcaster: SseBroadcaster,
}

#[routes]
impl SseController {
    /// SSE endpoint — clients subscribe to real-time events.
    #[sse("/events")]
    async fn events(&self) -> impl futures_core::Stream<Item = Result<SseEvent, Infallible>> {
        self.sse_broadcaster.subscribe()
    }
}
