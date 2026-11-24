---
topic: sse
features: core
tokens: ~900
requires: core-concepts
---

## SSE (Server-Sent Events)

### TL;DR

- Mark the method `#[sse("/path")]` and return a `Stream<Item = Result<SseEvent, Infallible>>`. Keep-alive is on by default; `#[sse("/path", keep_alive = 15)]` sets the interval in seconds, `keep_alive = false` disables it.
- Inject `SseBroadcaster` and return `self.sse_broadcaster.subscribe()`; the broadcaster is an ordinary bean — `.provide(SseBroadcaster::new(128))`.
- `SseTopic<E>` is the typed per-event topic fed by the EventBus→SSE bridge.
- One stream per entity (per-run logs, per-user feeds): inject `SseRooms<K>` and `subscribe(key)`; call `remove(&key)` / `reap_empty()` so rooms do not grow unbounded.
- A slow subscriber silently drops events by default; `subscribe_lagged("lagged")` emits a synthetic event carrying the dropped count instead.
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

### Sending, lag, rooms

All of these are in the prelude (`r2e::web::sse::*`).

| API | Purpose |
|-----|---------|
| `SseBroadcaster::new(cap)` | App-scoped broadcast channel (a bean); `subscriber_count()`, `capacity()` |
| `.send(data)` / `.send_event(name, data)` | Broadcast a data-only / named event; `Err` when nobody is subscribed |
| `.subscribe()` | `SseSubscription` (a `Stream<Item = Result<SseEvent, Infallible>>`); lagged events skipped silently |
| `.subscribe_lagged(name)` | Emit `event: <name>` with `data: <dropped count>` when the subscriber lags |
| `.subscribe_with(LagPolicy::Silent \| LagPolicy::Synthetic(name))` | The general form of the two above |
| `SseTopic<E>::new(cap)` | Typed topic: `publish(&e)` serializes `E`; `with_event_name(..)`, `subscribe*` as above |
| `SseRooms<K>::new(cap)` | Per-key broadcasters, created lazily: `room(key)` (the `SseBroadcaster`), `subscribe(key)` / `subscribe_lagged(key, name)` / `subscribe_with(key, policy)`, `remove(&key)`, `reap_empty() -> usize`, `room_count()` |

```rust
use std::convert::Infallible;
use r2e::http::response::SseEvent;
use r2e::web::sse::SseRooms;

#[controller(path = "/runs")]
pub struct RunLogController {
    #[inject] logs: SseRooms<String>,   // .provide(SseRooms::<String>::new(256))
}

#[routes]
impl RunLogController {
    #[sse("/{id}/logs", keep_alive = 15)]
    async fn logs(
        &self,
        Path(id): Path<String>,
    ) -> impl futures_core::Stream<Item = Result<SseEvent, Infallible>> {
        self.logs.subscribe_lagged(id, "lagged")
    }
}

// Producer side (a bean or a background job):
fn log_line(logs: &SseRooms<String>, run: &str, line: &str) {
    let _ = logs.room(run.to_string()).send_event("log", line);
}
# fn main() {}
```
