---
topic: sse
features: core
tokens: ~400
requires: core-concepts
---

## SSE (Server-Sent Events)

### TL;DR

- Mark the method `#[sse("/path")]` and return a `Stream<Item = Result<SseEvent, Infallible>>`.
- Inject `SseBroadcaster` and return `self.sse_broadcaster.subscribe()`; the broadcaster is an ordinary bean — `.provide(SseBroadcaster::new(128))`.
- `SseTopic<E>` is the typed per-event topic fed by the EventBus→SSE bridge.
- Streams from `#[sse]` are wrapped in `take_until(shutdown)` automatically — nothing to write or inject; a stream that must outlive the drain cannot use `#[sse]`.

```rust
use std::convert::Infallible;
use r2e::http::response::SseEvent;
use r2e::web::sse::SseBroadcaster;

#[controller(path = "/sse")]
pub struct SseController {
    #[inject] sse_broadcaster: SseBroadcaster,
    #[inject] user_events: SseTopic<UserCreatedEvent>,   // typed topic (see events bridge)
}

#[routes]
impl SseController {
    #[sse("/events")]
    async fn events(&self) -> impl futures_core::Stream<Item = Result<SseEvent, Infallible>> {
        self.sse_broadcaster.subscribe()
    }
}
# fn main() {}
```

Provide the broadcaster: `.provide(SseBroadcaster::new(128))`.

**`#[sse]` streams end at shutdown, by default.** The generated route wraps the
stream in `take_until(shutdown)` using the app's `rt::ShutdownToken` (resolved
once per route at registration, not per request), so an idle subscriber does not
hold the HTTP drain open until `drain_timeout` expires. Nothing to write in the
handler and nothing to inject; a stream that must outlive the drain has to be
served outside `#[sse]`.
