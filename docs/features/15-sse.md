# Feature 15 — Server-Sent Events (SSE)

## TL;DR

Server-Sent Events with the Axum boilerplate handled. Annotate a method `#[sse("/path")]` returning `impl Stream<Item = Result<SseEvent, Infallible>>` — the macro wraps it in `Sse::new()` with a default keep-alive. Inject an `SseBroadcaster` (wraps a `tokio::sync::broadcast` channel, `Clone + Send + Sync`) and push to every subscriber with `send()` / `send_event()`. No feature flag needed (available via the prelude).


## Goal

Provide native support for Server-Sent Events, allowing real-time updates to be pushed from the server to the client via a persistent HTTP connection in `text/event-stream` format. The `#[sse]` attribute and the `SseBroadcaster` abstraction handle all the Axum boilerplate.

## Key Concepts

### SseBroadcaster

`SseBroadcaster` is the central primitive for multi-client streaming. It wraps a `tokio::sync::broadcast` channel and is `Clone + Send + Sync`, making it injectable via `#[inject]`.

### SseSubscription

`SseSubscription` implements `futures_core::Stream<Item = Result<SseEvent, Infallible>>`, exactly what Axum's `Sse::new()` expects. It is returned directly from the `#[sse]` handler.

### SseMessage

Internal type sent through the broadcast channel:

```rust
pub struct SseMessage {
    pub event: Option<String>,  // nom du type d'evenement (champ SSE `event:`)
    pub data: String,           // donnees (champ SSE `data:`)
}
```

You generally do not construct `SseMessage` directly — use `SseBroadcaster::send()` and `send_event()` instead.

## Usage

### 1. Configuration

SSE support is included in `r2e-core` via the prelude. No additional feature flag is needed:

```toml
[dependencies]
r2e = { version = "0.1" }
```

### 2. `#[sse]` Attribute

Annotate a method in a `#[routes]` block with `#[sse("/path")]`. The method must return `impl Stream<Item = Result<SseEvent, Infallible>>`. The macro wraps the returned stream in `Sse::new()` and adds a default keep-alive.

```rust
use std::convert::Infallible;
use r2e::prelude::*;
use r2e::http::response::SseEvent;
use r2e::sse::SseBroadcaster;

#[controller(path = "/sse")]
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

### Keep-Alive Configuration

By default, `#[sse]` enables Axum's keep-alive (periodic comment frames to prevent proxies from closing idle connections). You can customize or disable it:

```rust
#[sse("/events", keep_alive = 15)]    // keep-alive toutes les 15 secondes
async fn events(&self) -> impl futures_core::Stream<Item = Result<SseEvent, Infallible>> {
    self.broadcaster.subscribe()
}

#[sse("/events", keep_alive = false)]  // desactiver le keep-alive
async fn events_no_ka(&self) -> impl futures_core::Stream<Item = Result<SseEvent, Infallible>> {
    self.broadcaster.subscribe()
}
```

### Path Parameters

SSE handlers accept the same Axum extractors as regular handlers. Useful for per-user or per-resource streams:

```rust
#[sse("/stream/{user_id}")]
async fn user_stream(
    &self,
    Path(user_id): Path<String>,
) -> impl futures_core::Stream<Item = Result<SseEvent, Infallible>> {
    self.notification_service.sse_broadcaster(&user_id).subscribe()
}
```

### 3. Creating an SseBroadcaster

```rust
use r2e::sse::SseBroadcaster;

// Creer avec une capacite de 128 messages
let broadcaster = SseBroadcaster::new(128);
```

The capacity determines the number of messages buffered for slow consumers. If a subscriber falls more than `capacity` messages behind, it skips the missed messages and resumes from the most recent.

### 4. Sending Messages

Two broadcasting methods are available:

```rust
// Evenement de donnees seul (sans type)
// Le client recoit : "data: hello\n\n"
broadcaster.send("hello").ok();

// Evenement type avec un nom
// Le client recoit : "event: update\ndata: {\"count\":42}\n\n"
broadcaster.send_event("update", r#"{"count":42}"#).ok();
```

Both methods return `Result<usize, SendError>` — on success, the `usize` is the number of subscribers the event reached. The error occurs only when there are no active subscribers. It can be ignored with `.ok()`.

### 5. Global Broadcaster (single shared stream)

Provide a single `SseBroadcaster` as a bean and inject it everywhere. There is no hand-written state struct — the state is an inferred HList, and beans are registered on the builder:

```rust
let broadcaster = SseBroadcaster::new(128);

AppBuilder::new()
    .provide(broadcaster)   // SseBroadcaster is now a bean, resolved by type
    // ...
```

Any service or controller that injects `SseBroadcaster` shares the same channel. (`R2eConfig` is itself a bean in the graph, so it needs no manual wiring.)

### 6. Per-Resource Broadcasters (user channels, rooms)

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

    /// Obtenir ou creer un broadcaster pour un utilisateur specifique.
    pub fn sse_broadcaster(&self, user_id: &str) -> SseBroadcaster {
        self.users
            .entry(user_id.to_string())
            .or_insert_with(|| SseBroadcaster::new(self.capacity))
            .clone()
    }

    /// Envoyer une notification sur le stream SSE d'un utilisateur.
    pub fn notify(&self, user_id: &str, message: &str) {
        if let Some(broadcaster) = self.users.get(user_id) {
            let _ = broadcaster.value().send_event("notification", message);
        }
    }
}
```

Integration in a controller:

```rust
#[controller(path = "/notifications")]
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

### 7. Typed Broadcast Topics (`SseTopic<E>`)

`SseTopic<E>` is a typed topic bean over `SseBroadcaster`: each event type `E`
makes it a distinct injectable type, so an app can provide several topics
without newtype boilerplate. `publish(&E)` serializes the event to JSON and
broadcasts it under the topic's SSE event name (default: the short type name
of `E`, override via `with_event_name`).

```rust
// app assembly — one topic per event type, no newtypes
.provide(SseTopic::<SyncStatus>::new(128))
.provide(SseTopic::<MatchProgress>::new(128).with_event_name("match"))
```

```rust
#[controller(path = "/sync")]
pub struct SyncController {
    #[inject]
    topic: SseTopic<SyncStatus>,
}

#[routes]
impl SyncController {
    #[sse("/status")]
    async fn status(&self) -> impl futures_core::Stream<Item = Result<SseEvent, Infallible>> {
        self.topic.subscribe()
    }
}
```

```rust
// anywhere (service, scheduled task, consumer)
topic.publish(&SyncStatus { done: 10, total: 42 })?;
```

Unlike `send_event`, publishing to a topic **with no active subscribers is
`Ok(0)`**, not an error — a topic nobody is watching is a normal state for a
publisher. `Err` means the event failed to serialize. `subscriber_count()`
and `capacity()` are exposed directly; `broadcaster()` is the raw escape
hatch (its `send`/`send_event` bypass the typed contract — prefer `publish`).

Payloads are JSON by default. SSE is a text protocol, so alternative formats
are text too — swap the serializer while keeping the typed `publish` contract
(and the EventBus bridge, which goes through `publish`):

```rust
let ticks = SseTopic::<Tick>::new(128)
    .with_event_name("tick")
    .with_serializer(|t| Ok(format!("{},{}", t.symbol, t.price)));
```

### 8. EventBus↔SSE Bridge (zero-liaison fan-out)

With the `events` feature, `bridge_sse::<Bus, E>()` (from
`r2e_events::SseBridgeExt`, in the prelude) forwards every event of type `E`
emitted on the EventBus into the provided `SseTopic<E>` — no `#[consumer]`
plumbing:

```rust
b.provide(LocalEventBus::new())
    .provide(SseTopic::<UserCreatedEvent>::new(64).with_event_name("user_created"))
    .register::<SyncController>()
    .build_state()
    .await
    .bridge_sse::<LocalEventBus, UserCreatedEvent>()
```

Services just `bus.emit(event)` as usual; SSE clients subscribed to the
topic's endpoint see it as JSON. With a distributed EventBus backend (Kafka,
RabbitMQ, …), SSE fan-out works across instances for free. The forwarding
subscription registers at server startup, alongside `#[consumer]` methods
(`TestApp::boot` also runs it, so the bridge is live in tests).

Both beans must be provided before `build_state()`; `bridge_sse` panics at
startup with a descriptive message otherwise. For manual wiring (e.g. outside
the builder), use `r2e_events::sse_bridge::bridge_event_to_sse(&bus, topic)`,
which is async and returns `Result<SubscriptionHandle, EventBusError>`.

## Complete Example

Minimal application with a global SSE stream:

```rust
use std::convert::Infallible;
use r2e::prelude::*;
use r2e::http::response::SseEvent;
use r2e::sse::SseBroadcaster;

// -- Controleur --

#[controller(path = "/sse")]
pub struct LiveController {
    #[inject]
    broadcaster: SseBroadcaster,
}

#[routes]
impl LiveController {
    /// Les clients se connectent ici pour recevoir des evenements en temps reel.
    #[sse("/events")]
    async fn events(&self) -> impl futures_core::Stream<Item = Result<SseEvent, Infallible>> {
        self.broadcaster.subscribe()
    }

    /// POST un message a diffuser a tous les clients connectes.
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

// -- Main --

#[tokio::main]
async fn main() {
    let broadcaster = SseBroadcaster::new(256);

    AppBuilder::new()
        .provide(broadcaster)
        .build_state()
        .await
        .with(ErrorHandling)
        .register_controller::<LiveController>()
        .serve_auto()
        .await
        .unwrap();
}
```

### Client Side (JavaScript)

```javascript
const source = new EventSource("/sse/events");

source.addEventListener("message", (e) => {
    console.log("evenement type:", e.data);
});

source.onmessage = (e) => {
    console.log("donnees:", e.data);
};

source.onerror = () => {
    console.log("connexion perdue, le navigateur se reconnecte automatiquement");
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

## API Reference

### SseBroadcaster

| Method | Signature | Description |
|--------|-----------|-------------|
| `new` | `fn new(capacity: usize) -> Self` | Create a broadcaster with the given buffer capacity |
| `send` | `fn send(&self, data: impl Into<String>) -> Result<usize, SendError>` | Broadcast a data-only event; `Ok` = subscriber count reached |
| `send_event` | `fn send_event(&self, event: &str, data: impl Into<String>) -> Result<usize, SendError>` | Broadcast a named event with data |
| `subscribe` | `fn subscribe(&self) -> SseSubscription` | Create a new subscription stream |

### SseTopic&lt;E&gt;

| Method | Signature | Description |
|--------|-----------|-------------|
| `new` | `fn new(capacity: usize) -> Self` | Topic with default event name = short type name of `E` |
| `with_event_name` | `fn with_event_name(self, name: impl Into<String>) -> Self` | Override the SSE event name |
| `with_serializer` | `fn with_serializer(self, f: impl Fn(&E) -> Result<String, SseSerializeError>) -> Self` | Override the payload format (default: JSON) |
| `publish` | `fn publish(&self, event: &E) -> Result<usize, SseSerializeError>` | Serialize and broadcast; `Ok(0)` when no subscribers |
| `subscribe` | `fn subscribe(&self) -> SseSubscription` | Subscription stream, ready for `#[sse]` |
| `subscriber_count` | `fn subscriber_count(&self) -> usize` | Active subscribers |
| `broadcaster` | `fn broadcaster(&self) -> &SseBroadcaster` | Raw escape hatch (prefer `publish`) |

### `#[sse]` Attribute

| Form | Description |
|------|-------------|
| `#[sse("/path")]` | SSE endpoint with default keep-alive |
| `#[sse("/path", keep_alive = N)]` | Custom keep-alive interval in seconds |
| `#[sse("/path", keep_alive = false)]` | Disable keep-alive |

## Validation Criteria

Launch the application and test the SSE stream:

```bash
# Terminal 1 : ecouter les evenements
curl -N http://localhost:3000/sse/events

# Terminal 2 : envoyer un message
curl -X POST http://localhost:3000/sse/broadcast \
  -H "Content-Type: application/json" \
  -d '{"text":"Bonjour le monde!"}'

# Terminal 1 affiche :
# event: message
# data: Bonjour le monde!
```
