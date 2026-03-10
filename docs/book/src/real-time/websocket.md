# WebSocket

R2E provides first-class WebSocket support built on top of Axum's WebSocket layer. The `#[ws]` attribute turns a controller method into a WebSocket endpoint with automatic upgrade handling, while `WsStream`, `WsBroadcaster`, and `WsRooms` give you ergonomic APIs for sending, receiving, and broadcasting messages.

## Setup

WebSocket support is included in `r2e-core` -- no extra feature flag is needed:

```rust
use r2e::prelude::*;
use r2e::ws::WsStream;
```

## Basic WebSocket endpoint

Annotate a controller method with `#[ws("/path")]` and accept a `WsStream` parameter. The framework handles the HTTP upgrade automatically:

```rust
#[derive(Controller)]
#[controller(path = "/ws", state = AppState)]
pub struct EchoController;

#[routes]
impl EchoController {
    #[ws("/echo")]
    async fn echo(&self, mut ws: WsStream) {
        ws.send_text("Welcome!").await.ok();
        ws.on_each(|msg| async move { Some(msg) }).await;
    }
}
```

The generated handler accepts a `WebSocketUpgrade` extractor, calls `on_upgrade`, wraps the raw socket in `WsStream`, and invokes your method. You never touch the upgrade machinery directly.

### Accepting additional extractors

WebSocket methods support the same Axum extractors as regular handlers. Place them before the `WsStream` parameter:

```rust
#[ws("/{room}")]
async fn join(
    &self,
    Path(room): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    mut ws: WsStream,
) {
    let username = params.get("username").cloned().unwrap_or_default();
    ws.send_text(format!("Hello {username}, welcome to {room}")).await.ok();
    // ...
}
```

## WsStream API

`WsStream` wraps Axum's `WebSocket` with typed helpers for common operations.

### Sending messages

| Method | Description |
|--------|-------------|
| `send(msg)` | Send a raw `Message` |
| `send_text(text)` | Send a text message |
| `send_json(&data)` | Serialize `data` as JSON and send as text |
| `send_binary(bytes)` | Send a binary message |

All send methods are async and return `Result<(), WsError>`.

```rust
ws.send_text("hello").await?;
ws.send_json(&MyPayload { value: 42 }).await?;
ws.send_binary(vec![0x01, 0x02]).await?;
```

### Receiving messages

| Method | Description |
|--------|-------------|
| `next()` | Receive the next raw `Message`, or `None` on close |
| `next_text()` | Receive the next text message, skipping non-text frames |
| `next_json::<T>()` | Receive and deserialize the next text message as `T` |

```rust
// Raw message loop
while let Some(Ok(msg)) = ws.next().await {
    match msg {
        Message::Text(t) => println!("got: {t}"),
        Message::Binary(b) => println!("binary: {} bytes", b.len()),
        Message::Close(_) => break,
        _ => {}
    }
}

// Typed JSON receive
while let Some(Ok(command)) = ws.next_json::<ClientCommand>().await {
    handle_command(command).await;
}
```

### Message loop helper

`on_each` runs a callback for every incoming message until the connection closes. Return `Some(msg)` to send a reply, or `None` to send nothing:

```rust
ws.on_each(|msg| async move {
    match msg {
        Message::Text(t) => Some(Message::Text(t.to_uppercase().into())),
        _ => None,
    }
}).await;
```

### Escape hatch

Call `into_inner()` to unwrap the raw Axum `WebSocket` if you need full control:

```rust
let raw: WebSocket = ws.into_inner();
```

## WsHandler trait

For structured WebSocket handling with lifecycle callbacks, implement `WsHandler`. The framework manages the message loop; you implement the hooks:

```rust
use r2e::ws::{WsHandler, WsStream};
use axum::extract::ws::Message;

struct ChatHandler {
    username: String,
}

impl WsHandler for ChatHandler {
    async fn on_connect(&mut self, ws: &mut WsStream) {
        ws.send_text(format!("{} connected", self.username)).await.ok();
    }

    async fn on_message(&mut self, ws: &mut WsStream, msg: Message) {
        if let Message::Text(text) = msg {
            let reply = format!("{}: {}", self.username, text);
            ws.send_text(reply).await.ok();
        }
    }

    async fn on_close(&mut self) {
        tracing::info!("{} disconnected", self.username);
    }
}
```

### WsHandler lifecycle

| Callback | When called | Required |
|----------|-------------|----------|
| `on_connect` | After WebSocket upgrade completes | No (default no-op) |
| `on_message` | For each received message (except `Close`) | Yes |
| `on_close` | After the connection closes | No (default no-op) |

### Using WsHandler in a controller

When a `#[ws]` method has no `WsStream` parameter, the framework expects it to return an `impl WsHandler`. The generated code calls `run_ws_handler` automatically:

```rust
#[derive(Controller)]
#[controller(path = "/ws", state = AppState)]
pub struct ChatController;

#[routes]
impl ChatController {
    #[ws("/chat")]
    fn chat_handler(&self, Query(params): Query<HashMap<String, String>>) -> ChatHandler {
        ChatHandler {
            username: params.get("username").cloned().unwrap_or_default(),
        }
    }
}
```

## WsBroadcaster

`WsBroadcaster` enables broadcasting messages to all connected WebSocket clients. It is `Clone + Send + Sync` and can be injected via `#[inject]`.

### Creating a broadcaster

```rust
use r2e::ws::WsBroadcaster;

let broadcaster = WsBroadcaster::new(128); // channel capacity
```

Add it to your state so it can be injected:

```rust
#[derive(Clone, BeanState)]
pub struct AppState {
    pub broadcaster: WsBroadcaster,
    // ...
}
```

### Broadcasting messages

| Method | Description |
|--------|-------------|
| `send(msg)` | Broadcast a raw `Message` to all subscribers |
| `send_text(text)` | Broadcast a text message |
| `send_json(&data)` | Broadcast a JSON-serialized message |
| `send_from(sender_id, msg)` | Broadcast, excluding the sender |
| `send_text_from(sender_id, text)` | Broadcast text, excluding the sender |
| `send_json_from(sender_id, &data)` | Broadcast JSON, excluding the sender |

The `_from` variants take a `sender_id` (obtained from `WsBroadcastReceiver::client_id()`) and skip delivery to that client. This prevents echo in chat-like scenarios.

### Subscribing and forwarding

Each WebSocket connection subscribes to the broadcaster and gets a `WsBroadcastReceiver`:

```rust
#[ws("/notifications")]
async fn notifications(&self, mut ws: WsStream) {
    let mut rx = self.broadcaster.subscribe();
    let client_id = rx.client_id();

    loop {
        tokio::select! {
            // Forward broadcasts to this client
            msg = rx.recv() => {
                match msg {
                    Some(msg) => {
                        if ws.send(msg).await.is_err() { break; }
                    }
                    None => break,
                }
            }
            // Read from this client and re-broadcast
            incoming = ws.next_text() => {
                match incoming {
                    Some(Ok(text)) => {
                        self.broadcaster.send_text_from(client_id, &text);
                    }
                    _ => break,
                }
            }
        }
    }
}
```

`recv()` automatically skips messages sent by the same client (matched by `sender_id`). If the receiver falls behind (lagged), missed messages are silently dropped and the receiver catches up.

## WsRooms

`WsRooms` manages named broadcasting rooms. Each room is a separate `WsBroadcaster`, created on demand. It is `Clone + Send + Sync` and injectable.

### Creating a room manager

```rust
use r2e::ws::WsRooms;

let rooms = WsRooms::new(128); // per-room channel capacity
```

Add it to your state:

```rust
#[derive(Clone, BeanState)]
pub struct AppState {
    pub ws_rooms: WsRooms,
    // ...
}
```

### Room API

| Method | Description |
|--------|-------------|
| `room(name)` | Get or create a `WsBroadcaster` for the named room |
| `remove(name)` | Remove a room |
| `room_count()` | Number of active rooms |

### Chat room example

This is a complete chat controller using `WsRooms` with JSON messages and event-driven persistence:

```rust
use r2e::prelude::*;
use r2e::ws::{WsRooms, WsStream};

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum WsIncoming {
    #[serde(rename = "message")]
    Message { text: String },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum WsOutgoing {
    #[serde(rename = "message")]
    Message { username: String, text: String, room: String },
    #[serde(rename = "join")]
    Join { username: String, room: String },
    #[serde(rename = "leave")]
    Leave { username: String, room: String },
}

#[derive(Controller)]
#[controller(path = "/chat", state = AppState)]
pub struct ChatController {
    #[inject]
    ws_rooms: WsRooms,
}

#[routes]
impl ChatController {
    #[ws("/{room}")]
    async fn join_room(
        &self,
        Path(room): Path<String>,
        Query(params): Query<HashMap<String, String>>,
        mut ws: WsStream,
    ) {
        let username = params
            .get("username")
            .cloned()
            .unwrap_or_else(|| "Anonymous".into());

        let broadcaster = self.ws_rooms.room(&room);
        let mut rx = broadcaster.subscribe();
        let client_id = rx.client_id();

        // Announce join
        let _ = broadcaster.send_json(&WsOutgoing::Join {
            username: username.clone(),
            room: room.clone(),
        });

        loop {
            tokio::select! {
                msg = rx.recv() => {
                    match msg {
                        Some(msg) => {
                            if ws.send(msg).await.is_err() { break; }
                        }
                        None => break,
                    }
                }
                incoming = ws.next_json::<WsIncoming>() => {
                    match incoming {
                        Some(Ok(WsIncoming::Message { text })) => {
                            let outgoing = WsOutgoing::Message {
                                username: username.clone(),
                                text,
                                room: room.clone(),
                            };
                            let _ = broadcaster.send_json_from(client_id, &outgoing);
                            let _ = ws.send_json(&outgoing).await;
                        }
                        Some(Err(_)) => continue,
                        None => break,
                    }
                }
            }
        }

        // Announce leave
        let _ = broadcaster.send_json(&WsOutgoing::Leave {
            username,
            room,
        });
    }
}
```

## WsError

WebSocket operations return `WsError` for error conditions:

| Variant | When |
|---------|------|
| `WsError::Send(axum::Error)` | Failed to send a message |
| `WsError::Recv(axum::Error)` | Failed to receive a message |
| `WsError::Json(serde_json::Error)` | JSON serialization or deserialization failed |
| `WsError::Closed` | The connection is closed |

`WsError` implements `Display` and `Error`.

## Guards on WebSocket endpoints

WebSocket endpoints support the same `#[guard]` attribute as HTTP handlers. Guards run before the upgrade, so a failed guard returns an HTTP error response (the WebSocket connection is never established):

```rust
#[ws("/admin")]
#[guard(AdminGuard)]
async fn admin_ws(&self, mut ws: WsStream) {
    // Only reachable if AdminGuard passes
}
```

## Summary

| Type | Purpose |
|------|---------|
| `#[ws("/path")]` | Declare a WebSocket endpoint on a controller method |
| `WsStream` | Ergonomic send/receive wrapper with text, JSON, and binary helpers |
| `WsHandler` | Lifecycle trait: `on_connect`, `on_message`, `on_close` |
| `WsBroadcaster` | Broadcast messages to all subscribed clients |
| `WsBroadcastReceiver` | Per-client receiver with sender exclusion |
| `WsRooms` | Named room manager, each room backed by a `WsBroadcaster` |
| `WsError` | Error type for WebSocket operations |

## Next steps

- [Event Bus](../events-and-scheduling/event-bus.md) -- combine WebSocket with events for persistence
- [Guards and Roles](../security/guards-and-roles.md) -- protect WebSocket endpoints
- [Interceptors](../advanced/interceptors.md) -- add cross-cutting concerns
