---
topic: websockets
features: ws
tokens: ~2900
requires: core-concepts
---

## WebSockets

### TL;DR

- Requires feature `ws`: mark the method `#[ws("/path")]` and take `mut ws: WsStream`.
- `send`/`send_text`/`send_json`/`send_binary` queue **and** flush; `feed`/`feed_text`/`feed_binary` queue only — you must call `flush()`, and you bound the batch size.
- `WsStream` implements `futures::Sink<Message>`, so `SinkExt` combinators (`send_all`, `forward`) work; `WsBroadcaster` / `WsRooms` give fan-out and rooms.
- Subprotocols: `#[ws("/path", protocols = ["v2", "v1"])]` (most preferred first) echoes the first one the client offered; the handler reads it with `ws.protocol()`.
- Size limits: `#[ws("/path", max_message_size = 64 * 1024, max_frame_size = ..)]` — each a `usize` expression or `"method"`, a plain sync `&self -> usize` method (config-driven bound); an oversized message closes the connection with a `1009` close frame, without being buffered.
- Guards, pre-auth guards and identity run **before** the upgrade: a refusal is an ordinary HTTP response in the app's error envelope, and `requested_protocols(ctx.headers)` reads what the client offered (a ticket carried as a subprotocol, say).
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
  (no-op when empty); `close()` flushes, sends a close frame and waits (at most 1 s) for the
  peer's; `close_with(code, reason)` does the same with an application close
  code (`ws.close_with(4001, "ticket expired").await?`). Close through these,
  not by returning from the handler or `send(Message::Close(..))`: without the
  handshake a peer that is still writing gets a TCP reset and may never see
  the close code.
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

### Subprotocols and refusal before the upgrade

`protocols = [..]` lists the subprotocols a route speaks, most preferred first
(`&'static str` expressions — literals or constants). The upgrade response
echoes the **first entry of that list the client offered**, and nothing else
of the client's `Sec-WebSocket-Protocol`; `ws.protocol() -> Option<&str>` is
what was selected.

A browser can set no header on a WebSocket except `Sec-WebSocket-Protocol`, so
a credential often travels there next to the real subprotocol
(`new WebSocket(url, ["collab.v1", "app.ticket.<ticket>"])`). The whole
pipeline — pre-auth guards, identity, guards, parameters — runs **before** the
upgrade, with the request headers at hand:

```rust
use r2e::web::ws::{requested_protocols, WsStream};

/// Judges the ticket before the upgrade.
pub struct TicketGate;
impl SelfBuilt for TicketGate {}
impl PreAuthGuard for TicketGate {
    async fn check(&self, ctx: &PreAuthGuardContext<'_>) -> Result<(), Rejection> {
        let ticket = requested_protocols(ctx.headers)
            .find_map(|protocol| protocol.strip_prefix("app.ticket."));
        match ticket {
            Some("good") => Ok(()),
            _ => Err(Rejection::forbidden("ticket refused")),
        }
    }
}

#[controller(path = "/collab")]
pub struct CollabController;

#[routes]
impl CollabController {
    #[ws("/open", protocols = ["collab.v2", "collab.v1"])]
    #[pre_guard(TicketGate)]
    async fn open(&self, mut ws: WsStream) {
        // "collab.v2" or "collab.v1" — never the ticket.
        let negotiated = ws.protocol().unwrap_or("none").to_string();
        ws.send_text(negotiated).await.ok();
    }
}
# fn main() {}
```

- `requested_protocols(&HeaderMap) -> impl Iterator<Item = &str>` — every
  subprotocol the client offered, in order, split on `,` and trimmed. Works
  from `GuardContext::headers` and `PreAuthGuardContext::headers` alike.
- **A refusal is plain HTTP.** A guard's `Err(Rejection)`, a failed identity
  or a bad path parameter on a `#[ws]` route never upgrades: it is rendered by
  the app-level envelope (`AppBuilder::error_projection::<E>()`, default
  `HttpError`) with its status — 401/403/…, no `Sec-WebSocket-Protocol`.
- **No match is not a refusal.** When the client offers subprotocols and none
  is in `protocols` (or the route declares none), the server still upgrades,
  without `Sec-WebSocket-Protocol`; a browser then fails the connection
  itself. `ws.protocol()` is `None` in the handler — close there, or reject
  the offer in a guard, to make it a server-side refusal.
- Negotiation happens on the upgrade, so it applies to every handler form:
  a `WsStream` parameter, a raw `WebSocket` parameter (its own `protocol()`),
  or a returned `WsHandler` (`ws.protocol()` in `on_connect`).
- An empty `protocols = []` and any `#[ws]` argument other than `protocols`,
  `max_message_size` and `max_frame_size` are compile errors.

In tests (`r2e-test`, feature `ws`):

```rust,ignore
let server = app.serve().await;
let mut ws = server.ws_with_protocols("/collab/open", &["collab.v1", "app.ticket.good"]).await;
assert_eq!(ws.protocol(), Some("collab.v1"));

// A refusal: the handshake fails with the HTTP response.
let url = format!("{}/collab/open", server.ws_url());
let error = WsTestClient::connect_with_protocols(&url, &["collab.v1"]).await.err().unwrap();
assert_eq!(error.http_status(), Some(StatusCode::FORBIDDEN));
// The body (the envelope) is asserted in-process: app.get("/collab/open")
//   .header("sec-websocket-protocol", "collab.v1, app.ticket.bad").send().await
```

Like a browser, the test client fails a handshake that offered subprotocols
and got none back.

### Message and frame size limits

`max_message_size` and `max_frame_size` bound, in bytes, what a session
accepts from the client. Both are optional; a route that sets neither keeps the
defaults (64 MiB per message, 16 MiB per frame). Each takes either a `usize`
expression (a literal, a `const`) or a string naming a **plain, sync
`&self -> usize` method of the same impl block** — the way to read the bound
from config or a bean. The method is called at each upgrade, after guards and
identity.

```rust
use r2e::prelude::*;
use r2e::web::ws::WsStream;

const VOICE_FRAME: usize = 64 * 1024;

#[controller(path = "/rt")]
pub struct RealtimeController {
    #[config("collab.max-message-size")]
    collab_max_message_size: usize,
}

#[routes]
impl RealtimeController {
    fn collab_limit(&self) -> usize {
        self.collab_max_message_size
    }

    // Fixed bound.
    #[ws("/voice", max_message_size = VOICE_FRAME)]
    async fn voice(&self, mut ws: WsStream) {
        ws.on_each(|msg| async move { Some(msg) }).await;
    }

    // Bound read from `collab.max-message-size`; combines with `protocols`.
    #[ws("/collab", protocols = ["collab.v1"], max_message_size = "collab_limit")]
    async fn collab(&self, mut ws: WsStream) {
        ws.on_each(|msg| async move { Some(msg) }).await;
    }
}
# fn main() {}
```

- **Over the bound, the connection ends.** The read fails
  (`ws.next()` yields `Some(Err(_))` then `None`, `on_each` returns) and the
  client receives a `1009` Message Too Big close frame
  (`r2e::web::ws::CLOSE_MESSAGE_TOO_BIG`); the oversized message is never
  delivered to the handler.
- **It is not buffered first.** With `max_message_size` set, the frame limit
  is clamped to it (`min(max_frame_size or 16 MiB, max_message_size)`), so an
  oversized single-frame message is refused on its frame header, before its
  payload is read. A message split into small frames is cut as soon as the
  accumulated size passes the bound.
- `max_frame_size` alone bounds each frame and leaves the message default.
- The limits apply to what the server **reads**; they do not bound what the
  handler sends.
- A string that names no plain method, an `async` method, or a value that is
  not `usize` is a compile error on the attribute.

In tests, `WsTestClient::assert_closed()` waits for the server to end the
connection:

```rust,ignore
let mut ws = server.ws("/rt/voice").await;
ws.send_binary(vec![0u8; 64 * 1024 + 1]).await.unwrap();
ws.assert_closed().await;
```

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
