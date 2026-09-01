---
topic: websockets
features: ws
tokens: ~1100
requires: core-concepts
---

## WebSockets

### TL;DR

- Requires feature `ws`: mark the method `#[ws("/path")]` and take `mut ws: WsStream`.
- `send`/`send_text`/`send_json`/`send_binary` queue **and** flush; `feed`/`feed_text`/`feed_binary` queue only — you must call `flush()`, and you bound the batch size.
- `WsStream` implements `futures::Sink<Message>`, so `SinkExt` combinators (`send_all`, `forward`) work; `WsBroadcaster` / `WsRooms` give fan-out and rooms.
- Every `#[ws]` session is a tracked task joined at shutdown under `shutdown_grace_period` (it does not hold the HTTP drain).
- A plain `while let Some(Ok(msg)) = ws.next().await` loop already exits cleanly: at shutdown `next()` sends `1001 Going Away` and returns `None`.
- On the send side, `select!` on `ws.shutdown_requested()` (a `'static` future, safe to hold across awaits); `ws.shutdown_token()` gives the token, `CLOSE_GOING_AWAY` the `1001` constant.
- A handler that ignores the signal is detached — never aborted — once the grace period elapses, with a named warning.
- Under `TestApp` / `build_with_consumers()` sessions run untracked and `shutdown_token()` is `None`.

Requires feature: `ws`

```rust
use r2e::web::ws::WsStream;

#[controller(path = "/ws")]
pub struct WsEchoController;

#[routes]
impl WsEchoController {
    #[ws("/echo")]
    async fn echo(&self, mut ws: WsStream) {
        ws.send_text("Welcome!").await.ok();
        ws.on_each(|msg| async move { Some(msg) }).await;
    }
}
# fn main() {}
```

`WsStream` send API — two tiers, both `async -> Result<(), WsError>`:

- `send(msg)` / `send_text` / `send_json(&T)` / `send_binary(impl Into<Bytes>)` —
  queue one frame **and flush** (on the wire when the future resolves).
- `feed(msg)` / `feed_text` / `feed_binary` — queue **without** flushing (awaits
  sink readiness, honours backpressure); `flush()` writes everything queued
  (no-op when empty); `close()` flushes then sends the close handshake.
  `send(msg)` == `feed(msg)` + `flush()`. No timer, no writer task, no implicit
  flush; frame order preserved. **The caller bounds the batch** (frames/bytes).

```rust
# async fn __doc(mut ws: WsStream, batch: Vec<Message>) -> Result<(), WsError> {
for message in batch {          // e.g. ≤ 16 frames / 256 KiB
    ws.feed(message).await?;    // Bytes payloads are moved, never copied
}
ws.flush().await?;
# Ok(()) }
```

`WsStream` implements `futures::Sink<Message>` (`Error = WsError`), so `SinkExt`
combinators (`send_all`, `forward`) work directly.

`WsBroadcaster` / `WsRooms` (prelude) support fan-out and room semantics.

### Sessions and shutdown

Every session opened by a `#[ws(...)]` route is a **tracked task**: it owns the
bean graph while it runs and is joined during shutdown alongside
`spawn_service` / `ServeContext::track` handles, bounded per session by
`shutdown_grace_period` and named `ws:<Controller>::<method>` in the overflow
warning. (It does not hold the HTTP drain — for hyper the connection ended at
the upgrade.)

`WsStream` observes the app shutdown token, so the ordinary loop already exits
cleanly:

```rust
#[controller(path = "/ws")]
pub struct WsChatController;

#[routes]
impl WsChatController {
    #[ws("/chat")]
    async fn chat(&self, mut ws: WsStream) {
        while let Some(Ok(msg)) = ws.next().await {
            let _ = msg;
        }
        // At shutdown `next()` sends `1001 Going Away` and returns None.
    }
}
# fn main() {}
```

- `ws.shutdown_requested().await` — the same signal as a bare future (pending
  forever when the app is not served through `run()`), for a `select!` on the
  send side. It borrows nothing (`impl Future<Output = ()> + Send + 'static`),
  so it can be held across awaits without making the session future non-`Send`.
- `ws.shutdown_token() -> Option<&CancelToken>` — the token itself.
- `r2e::web::ws::CLOSE_GOING_AWAY` — the `1001` constant.

A handler that ignores the signal is abandoned (detached, not aborted) once
`shutdown_grace_period` elapses, with the named warning; `on_stop` still runs.
Under `TestApp` / `build_with_consumers()` (never served through `run()`)
sessions run untracked with `shutdown_token() == None`, exactly as before —
`TestApp::boot` runs the startup and shutdown phases, not the serve hooks that
arm the session registry.

Sharded serving (`server.workers`) behaves identically: worker runtimes are
kept alive until the tracked-handle join is over, so a session's socket stays
writable for its whole grace period even though it was accepted by a worker and
runs on the control plane.
