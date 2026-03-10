# Server-Sent Events (SSE)

R2E provides first-class support for Server-Sent Events through the `#[sse]` attribute and the `SseBroadcaster` abstraction. SSE lets you push real-time updates from server to client over a long-lived HTTP connection, using the standard `text/event-stream` content type.

The macro handles all the Axum boilerplate: your handler returns a `Stream` and R2E wraps it in `Sse::new()` with keep-alive automatically.

## Setup

SSE support lives in `r2e-core` and is available through the prelude. No extra feature flag is needed.

```toml
[dependencies]
r2e = { version = "0.1" }
```

## The `#[sse]` attribute

Annotate a method inside a `#[routes]` block with `#[sse("/path")]`. The method must return `impl Stream<Item = Result<SseEvent, Infallible>>`. The macro wraps the returned stream in `Sse::new()` and adds a default keep-alive.

```rust
use std::convert::Infallible;
use r2e::prelude::*;
use r2e::http::response::SseEvent;
use r2e::sse::SseBroadcaster;

#[derive(Controller)]
#[controller(path = "/sse", state = AppState)]
pub struct SseController {
    #[inject]
    broadcaster: SseBroadcaster,
}

#[routes]
impl SseController {
    #[sse("/events")]
    async fn events(&self) -> impl futures_core::Stream<Item = Result<SseEvent, Infallible>> {
        self.broadcaster.subscribe()
    }
}
```

This registers a `GET /sse/events` endpoint that returns `Content-Type: text/event-stream`.

### Keep-alive configuration

By default, `#[sse]` enables Axum's default keep-alive (periodic comment frames to prevent proxies from closing idle connections). You can customize or disable it:

```rust
#[sse("/events", keep_alive = 15)]    // send keep-alive every 15 seconds
async fn events(&self) -> impl futures_core::Stream<Item = Result<SseEvent, Infallible>> {
    self.broadcaster.subscribe()
}

#[sse("/events", keep_alive = false)]  // disable keep-alive entirely
async fn events_no_ka(&self) -> impl futures_core::Stream<Item = Result<SseEvent, Infallible>> {
    self.broadcaster.subscribe()
}
```

### Path parameters

SSE handlers accept the same Axum extractors as regular handlers. This is useful for per-user or per-resource streams:

```rust
#[sse("/stream/{user_id}")]
async fn user_stream(
    &self,
    Path(user_id): Path<String>,
) -> impl futures_core::Stream<Item = Result<SseEvent, Infallible>> {
    self.notification_service.sse_broadcaster(&user_id).subscribe()
}
```

## SseBroadcaster

`SseBroadcaster` is the core primitive for multi-client streaming. It wraps a `tokio::sync::broadcast` channel and is `Clone + Send + Sync`, making it suitable for `#[inject]` fields, shared state, and cross-service communication.

### Creating a broadcaster

```rust
use r2e::sse::SseBroadcaster;

// Create with a channel capacity of 128 messages
let broadcaster = SseBroadcaster::new(128);
```

The capacity determines how many messages are buffered for slow consumers. If a subscriber falls behind by more than `capacity` messages, it skips the missed messages and resumes from the latest.

### Sending messages

Two methods are available for broadcasting:

```rust
// Data-only event (no event type)
// Client receives: "data: hello\n\n"
broadcaster.send("hello").ok();

// Typed event with a name
// Client receives: "event: update\ndata: {\"count\":42}\n\n"
broadcaster.send_event("update", r#"{"count":42}"#).ok();
```

Both methods return `Result<(), SendError>`. The error occurs only when there are zero active subscribers. It is safe to ignore with `.ok()`.

### Subscribing

```rust
let subscription: SseSubscription = broadcaster.subscribe();
```

Each call to `subscribe()` creates an independent `SseSubscription` stream. Multiple clients can subscribe to the same broadcaster; each receives every message sent after their subscription.

## SseSubscription

`SseSubscription` implements `futures_core::Stream<Item = Result<SseEvent, Infallible>>`, which is exactly what Axum's `Sse::new()` expects. You do not need to interact with it directly -- just return it from your `#[sse]` handler.

The stream:
- Yields events as they arrive on the broadcast channel.
- Skips messages if the subscriber falls behind (lagged), then resumes.
- Terminates when the broadcaster is dropped (all senders gone).

## SseMessage

`SseMessage` is the internal message type sent through the broadcast channel:

```rust
#[derive(Clone, Debug)]
pub struct SseMessage {
    /// Optional event type name (maps to the SSE `event:` field).
    pub event: Option<String>,
    /// Event data payload (maps to the SSE `data:` field).
    pub data: String,
}
```

You typically do not construct `SseMessage` directly. Use `SseBroadcaster::send()` and `send_event()` instead.

## Registering the broadcaster

### Global broadcaster (single shared stream)

Register a single `SseBroadcaster` as a bean and inject it wherever needed:

```rust
#[derive(Clone, BeanState)]
pub struct AppState {
    pub config: R2eConfig,
    pub sse_broadcaster: SseBroadcaster,
}
```

Build it during app startup:

```rust
let broadcaster = SseBroadcaster::new(128);

let state = AppState {
    config,
    sse_broadcaster: broadcaster.clone(),
};
```

Any service or controller that injects `SseBroadcaster` shares the same channel.

### Per-resource broadcasters (user channels, rooms)

For per-user or per-topic streams, manage a map of broadcasters in a service:

```rust
use std::sync::Arc;
use dashmap::DashMap;
use r2e::sse::SseBroadcaster;

#[derive(Clone)]
pub struct NotificationService {
    users: Arc<DashMap<String, SseBroadcaster>>,
    capacity: usize,
}

impl NotificationService {
    pub fn new(capacity: usize) -> Self {
        Self {
            users: Arc::new(DashMap::new()),
            capacity,
        }
    }

    /// Get or create a broadcaster for a specific user.
    pub fn sse_broadcaster(&self, user_id: &str) -> SseBroadcaster {
        self.users
            .entry(user_id.to_string())
            .or_insert_with(|| SseBroadcaster::new(self.capacity))
            .clone()
    }

    /// Send a notification to a specific user's SSE stream.
    pub fn notify(&self, user_id: &str, message: &str) {
        if let Some(broadcaster) = self.users.get(user_id) {
            let _ = broadcaster.value().send_event("notification", message);
        }
    }
}
```

Wire this into a controller:

```rust
#[derive(Controller)]
#[controller(path = "/notifications", state = AppState)]
pub struct NotificationController {
    #[inject]
    notification_service: NotificationService,
}

#[routes]
impl NotificationController {
    #[sse("/sse/{user_id}")]
    async fn sse_subscribe(
        &self,
        Path(user_id): Path<String>,
    ) -> impl futures_core::Stream<Item = Result<SseEvent, Infallible>> {
        self.notification_service.sse_broadcaster(&user_id).subscribe()
    }

    #[post("/send/{user_id}")]
    async fn send_to_user(
        &self,
        Path(user_id): Path<String>,
        Json(body): Json<SendNotification>,
    ) -> Json<NotificationResult> {
        self.notification_service.notify(&user_id, &body.message);
        Json(NotificationResult {
            sent_to: user_id,
            message: body.message,
        })
    }
}
```

## Complete example

A minimal app with a global SSE stream:

```rust
use std::convert::Infallible;
use r2e::prelude::*;
use r2e::http::response::SseEvent;
use r2e::sse::SseBroadcaster;

// ── State ──

#[derive(Clone, BeanState)]
pub struct AppState {
    pub config: R2eConfig,
    pub broadcaster: SseBroadcaster,
}

// ── Controller ──

#[derive(Controller)]
#[controller(path = "/sse", state = AppState)]
pub struct LiveController {
    #[inject]
    broadcaster: SseBroadcaster,
}

#[routes]
impl LiveController {
    /// Clients connect here to receive real-time events.
    #[sse("/events")]
    async fn events(&self) -> impl futures_core::Stream<Item = Result<SseEvent, Infallible>> {
        self.broadcaster.subscribe()
    }

    /// POST a message to broadcast to all connected clients.
    #[post("/broadcast")]
    async fn broadcast(&self, Json(body): Json<BroadcastRequest>) -> StatusCode {
        let _ = self.broadcaster.send_event("message", &body.text);
        StatusCode::NO_CONTENT
    }
}

#[derive(serde::Deserialize)]
struct BroadcastRequest {
    text: String,
}

// ── Main ──

#[tokio::main]
async fn main() {
    let config = R2eConfig::load().unwrap();
    let broadcaster = SseBroadcaster::new(256);

    let state = AppState {
        config: config.clone(),
        broadcaster,
    };

    AppBuilder::new()
        .with_config(config)
        .with_state(state)
        .with(ErrorHandling)
        .register_controller::<LiveController>()
        .serve_auto()
        .await
        .unwrap();
}
```

### Client-side JavaScript

```javascript
const source = new EventSource("/sse/events");

source.addEventListener("message", (e) => {
    console.log("typed event:", e.data);
});

source.onmessage = (e) => {
    console.log("data:", e.data);
};

source.onerror = () => {
    console.log("connection lost, browser will auto-reconnect");
};
```

## Decorators

SSE endpoints support the same decorators as regular routes:

- **Guards:** `#[guard(MyGuard)]` to restrict access.
- **Roles:** `#[roles("ADMIN")]` for role-based access control.
- **Middleware:** `#[middleware(my_middleware)]` for custom Axum middleware.
- **Interceptors:** `#[intercept(Logged::info())]` at the impl level.

```rust
#[routes]
impl SseController {
    #[sse("/admin/events")]
    #[roles("ADMIN")]
    async fn admin_events(&self) -> impl futures_core::Stream<Item = Result<SseEvent, Infallible>> {
        self.broadcaster.subscribe()
    }
}
```

## API reference

### `SseBroadcaster`

| Method | Signature | Description |
|--------|-----------|-------------|
| `new` | `fn new(capacity: usize) -> Self` | Create a broadcaster with the given buffer capacity |
| `send` | `fn send(&self, data: impl Into<String>) -> Result<(), SendError>` | Broadcast a data-only event |
| `send_event` | `fn send_event(&self, event: &str, data: impl Into<String>) -> Result<(), SendError>` | Broadcast a named event with data |
| `subscribe` | `fn subscribe(&self) -> SseSubscription` | Create a new subscription stream |

### `SseMessage`

| Field | Type | Description |
|-------|------|-------------|
| `event` | `Option<String>` | Event type name (SSE `event:` field) |
| `data` | `String` | Event payload (SSE `data:` field) |

### `#[sse]` attribute

| Form | Description |
|------|-------------|
| `#[sse("/path")]` | SSE endpoint with default keep-alive |
| `#[sse("/path", keep_alive = N)]` | Custom keep-alive interval in seconds |
| `#[sse("/path", keep_alive = false)]` | Disable keep-alive |
