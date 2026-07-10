use std::convert::Infallible;

use r2e::prelude::*;
use r2e::http::response::SseEvent;
use r2e::sse::SseBroadcaster;

use crate::models::UserCreatedEvent;

#[controller(path = "/sse")]
pub struct SseController {
    #[inject]
    sse_broadcaster: SseBroadcaster,
    #[inject]
    user_events: SseTopic<UserCreatedEvent>,
}

#[routes]
impl SseController {
    /// SSE endpoint — clients subscribe to real-time events.
    #[sse("/events")]
    async fn events(&self) -> impl futures_core::Stream<Item = Result<SseEvent, Infallible>> {
        self.sse_broadcaster.subscribe()
    }

    /// Typed topic fed by the EventBus↔SSE bridge: every `UserCreatedEvent`
    /// emitted on the bus (e.g. by `POST /users/`) is broadcast here as JSON,
    /// with zero liaison code (see `bridge_sse` in the blueprint).
    #[sse("/users")]
    async fn user_created(&self) -> impl futures_core::Stream<Item = Result<SseEvent, Infallible>> {
        self.user_events.subscribe()
    }
}
