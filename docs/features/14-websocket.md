# Feature 14 — WebSocket

## Goal

Provide native WebSocket support within R2E controllers. The `#[ws]` attribute transforms a controller method into a WebSocket endpoint with automatic HTTP upgrade handling. The `WsStream`, `WsBroadcaster`, and `WsRooms` types offer an ergonomic API for sending, receiving, and broadcasting messages.

## Key Concepts

### WsStream

`WsStream` wraps Axum's `WebSocket` with typed helpers for common operations (text, JSON, binary). You never directly handle the upgrade mechanics.

### WsBroadcaster

`WsBroadcaster` broadcasts messages to all connected WebSocket clients. It is `Clone + Send + Sync` and can be injected via `#[inject]`.

### WsRooms

`WsRooms` manages named broadcast rooms. Each room is a distinct `WsBroadcaster`, created on demand.

### WsHandler

A lifecycle trait (`on_connect`, `on_message`, `on_close`) for structured WebSocket connection management.

## Usage

### 1. Configuration

WebSocket support is included in `r2e-core` — no additional feature flag is needed:

```rust
use r2e::prelude::*;
use r2e::ws::WsStream;
```

### 2. Basic WebSocket Endpoint

Annotate a controller method with `#[ws("/path")]` and accept a `WsStream` parameter:

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

The generated handler accepts a `WebSocketUpgrade`, calls `on_upgrade`, wraps the raw socket in a `WsStream`, and invokes the method. The upgrade is entirely transparent.

### Additional Extractors

WebSocket methods support the same Axum extractors as HTTP handlers. They are placed before the `WsStream` parameter:

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

### 3. WsStream API

#### Sending Messages

| Method | Description |
|--------|-------------|
| `send(msg)` | Send a raw `Message` |
| `send_text(text)` | Send a text message |
| `send_json(&data)` | Serialize `data` to JSON and send as text |
| `send_binary(bytes)` | Send a binary message |

All send methods are async and return `Result<(), WsError>`.

```rust
ws.send_text("hello").await?;
ws.send_json(&MyPayload { value: 42 }).await?;
ws.send_binary(vec![0x01, 0x02]).await?;
```

#### Receiving Messages

| Method | Description |
|--------|-------------|
| `next()` | Receive the next raw `Message`, or `None` on close |
| `next_text()` | Receive the next text message, skipping non-text frames |
| `next_json::<T>()` | Receive and deserialize the next text message into `T` |

```rust
// Boucle de messages bruts
while let Some(Ok(msg)) = ws.next().await {
    match msg {
        Message::Text(t) => println!("recu: {t}"),
        Message::Binary(b) => println!("binaire: {} octets", b.len()),
        Message::Close(_) => break,
        _ => {}
    }
}

// Reception JSON typee
while let Some(Ok(command)) = ws.next_json::<ClientCommand>().await {
    handle_command(command).await;
}
```

#### Message Loop Helper

`on_each` executes a callback for each incoming message until the connection closes. Returning `Some(msg)` sends a response, `None` sends nothing:

```rust
ws.on_each(|msg| async move {
    match msg {
        Message::Text(t) => Some(Message::Text(t.to_uppercase().into())),
        _ => None,
    }
}).await;
```

#### Raw Access

Call `into_inner()` to retrieve the raw Axum `WebSocket` if full control is needed:

```rust
let raw: WebSocket = ws.into_inner();
```

### 4. WsHandler Trait

For structured management with lifecycle callbacks, implement `WsHandler`:

```rust
use r2e::ws::{WsHandler, WsStream};
use axum::extract::ws::Message;

struct ChatHandler {
    username: String,
}

impl WsHandler for ChatHandler {
    async fn on_connect(&mut self, ws: &mut WsStream) {
        ws.send_text(format!("{} connecte", self.username)).await.ok();
    }

    async fn on_message(&mut self, ws: &mut WsStream, msg: Message) {
        if let Message::Text(text) = msg {
            let reply = format!("{}: {}", self.username, text);
            ws.send_text(reply).await.ok();
        }
    }

    async fn on_close(&mut self) {
        tracing::info!("{} deconnecte", self.username);
    }
}
```

#### WsHandler Lifecycle

| Callback | When | Required |
|----------|------|----------|
| `on_connect` | After WebSocket upgrade completion | No (no-op by default) |
| `on_message` | For each received message (except `Close`) | Yes |
| `on_close` | After the connection is closed | No (no-op by default) |

#### Using WsHandler in a Controller

When a `#[ws]` method has no `WsStream` parameter, the framework expects it to return an `impl WsHandler`:

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

### 5. WsBroadcaster — Broadcasting to All Clients

#### Creation

```rust
use r2e::ws::WsBroadcaster;

let broadcaster = WsBroadcaster::new(128); // capacite du canal
```

Add to the state for injection:

```rust
#[derive(Clone, BeanState)]
pub struct AppState {
    pub broadcaster: WsBroadcaster,
    // ...
}
```

#### Broadcasting Methods

| Method | Description |
|--------|-------------|
| `send(msg)` | Broadcast a raw `Message` to all subscribers |
| `send_text(text)` | Broadcast a text message |
| `send_json(&data)` | Broadcast a serialized JSON message |
| `send_from(sender_id, msg)` | Broadcast excluding the sender |
| `send_text_from(sender_id, text)` | Broadcast text excluding the sender |
| `send_json_from(sender_id, &data)` | Broadcast JSON excluding the sender |

The `_from` variants take a `sender_id` (obtained via `WsBroadcastReceiver::client_id()`) and skip delivery to that client. This avoids echo in chat-like scenarios.

#### Subscription and Relay

Each WebSocket connection subscribes to the broadcaster and receives a `WsBroadcastReceiver`:

```rust
#[ws("/notifications")]
async fn notifications(&self, mut ws: WsStream) {
    let mut rx = self.broadcaster.subscribe();
    let client_id = rx.client_id();

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

`recv()` automatically skips messages sent by the same client (matching by `sender_id`). If the receiver falls behind, missed messages are silently dropped.

### 6. WsRooms — Room Management

#### Creation

```rust
use r2e::ws::WsRooms;

let rooms = WsRooms::new(128); // capacite par salle
```

Add to the state:

```rust
#[derive(Clone, BeanState)]
pub struct AppState {
    pub ws_rooms: WsRooms,
    // ...
}
```

#### Room API

| Method | Description |
|--------|-------------|
| `room(name)` | Get or create a `WsBroadcaster` for the named room |
| `remove(name)` | Remove a room |
| `room_count()` | Number of active rooms |

#### Room-Based Chat Example

Complete controller using `WsRooms` with JSON messages:

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

        // Annoncer l'arrivee
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

        // Annoncer le depart
        let _ = broadcaster.send_json(&WsOutgoing::Leave {
            username,
            room,
        });
    }
}
```

## WsError

WebSocket operations return `WsError` on failure:

| Variant | When |
|---------|------|
| `WsError::Send(axum::Error)` | Failed to send a message |
| `WsError::Recv(axum::Error)` | Failed to receive a message |
| `WsError::Json(serde_json::Error)` | JSON serialization or deserialization error |
| `WsError::Closed` | The connection is closed |

`WsError` implements `Display` and `Error`.

## Guards on WebSocket Endpoints

WebSocket endpoints support the same `#[guard]` attribute as HTTP handlers. Guards run before the upgrade, so a failing guard returns an HTTP error response (the WebSocket connection is never established):

```rust
#[ws("/admin")]
#[guard(AdminGuard)]
async fn admin_ws(&self, mut ws: WsStream) {
    // Accessible uniquement si AdminGuard passe
}
```

## Summary

| Type | Role |
|------|------|
| `#[ws("/path")]` | Declare a WebSocket endpoint on a controller method |
| `WsStream` | Ergonomic send/receive wrapper with text, JSON, and binary helpers |
| `WsHandler` | Lifecycle trait: `on_connect`, `on_message`, `on_close` |
| `WsBroadcaster` | Broadcast messages to all subscribed clients |
| `WsBroadcastReceiver` | Per-client receiver with sender exclusion |
| `WsRooms` | Named room manager, each room backed by a `WsBroadcaster` |
| `WsError` | Error type for WebSocket operations |

## Validation Criteria

Launch the application and connect via WebSocket:

```bash
# Avec websocat ou un client WebSocket
websocat ws://localhost:3000/ws/echo
# → Envoyer un message, recevoir l'echo
```
