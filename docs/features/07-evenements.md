# Feature 7 — Events

## Objective

Provide an in-process event bus with typed pub/sub. Allows decoupling application components by emitting events that other parts can listen to.

## Key Concepts

### EventBus (trait) and LocalEventBus

`EventBus` is a trait defining the interface of an event bus. `LocalEventBus` is the default implementation (in-process). It is `Clone` and can be shared between threads. Dispatch is based on `TypeId` — each event type has its own subscribers.

You can implement the `EventBus` trait for custom backends (Kafka, Redis, NATS, etc.).

### Strong typing

Events are dispatched by Rust type. A subscriber to `UserCreatedEvent` will never receive an `OrderPlacedEvent`. No magic strings, no manual downcasting.

## Usage

### 1. Add the dependency

```toml
[dependencies]
r2e-events = { path = "../r2e-events" }
```

### 2. Define an event type

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UserCreatedEvent {
    pub user_id: u64,
    pub name: String,
    pub email: String,
}
```

The type must be `Send + Sync + Serialize + DeserializeOwned + 'static`. The serde bounds are required by the `EventBus` trait (for compatibility with remote backends), but `LocalEventBus` never serializes — zero overhead.

### 3. Create the bus and subscribe

```rust
use std::sync::Arc;
use r2e_events::{EventBus, LocalEventBus};

let event_bus = LocalEventBus::new();

// Subscribe to an event type
event_bus
    .subscribe(|event: Arc<UserCreatedEvent>| async move {
        tracing::info!(
            user_id = event.user_id,
            name = %event.name,
            email = %event.email,
            "Nouvel utilisateur cree"
        );
    })
    .await;
```

**Note**: the handler receives `Arc<E>` (not `E` directly), because the event may be shared among multiple subscribers.

### Multiple subscribers

Multiple handlers can listen to the same type:

```rust
// Handler 1: log
event_bus.subscribe(|event: Arc<UserCreatedEvent>| async move {
    tracing::info!("User created: {}", event.name);
}).await;

// Handler 2: email notification
event_bus.subscribe(|event: Arc<UserCreatedEvent>| async move {
    send_welcome_email(&event.email).await;
}).await;

// Handler 3: analytics
event_bus.subscribe(|event: Arc<UserCreatedEvent>| async move {
    track_signup(event.user_id).await;
}).await;
```

### 4. Emit an event

```rust
// Fire-and-forget emission (handlers run as parallel Tokio tasks)
event_bus.emit(UserCreatedEvent {
    user_id: 42,
    name: "Alice".into(),
    email: "alice@example.com".into(),
}).await;
```

`emit` is fan-out publish/subscribe (Vert.x `publish` semantics): every
subscriber receives a copy, the emitter never waits for handlers and cannot
observe a reply.

### Request-reply: `request` / `respond`

When you need a **result** back, use the point-to-point request-reply API
(Vert.x `request` semantics) instead of waiting on subscribers:

```rust
// Responder side — at most one responder per request type per process.
event_bus.respond(|envelope: EventEnvelope<GreetRequest>| async move {
    Ok::<_, String>(GreetReply { message: format!("Hello {}", envelope.event.name) })
}).await?;

// Requester side — exactly one responder replies, with a timeout (30s default).
let reply: GreetReply = event_bus.request(GreetRequest { name: "Alice".into() }).await?;

// Explicit timeout
let reply: GreetReply = event_bus
    .request_with(GreetRequest { name: "Alice".into() },
                  RequestOptions::new().with_timeout(Duration::from_secs(5)))
    .await?;
```

A responder error surfaces to the caller as `EventBusError::Remote`;
`NoResponder` is only detectable in-process (on distributed backends an
absent responder manifests as `RequestTimeout`). In controllers, a
`#[consumer]` method with a non-`()` return type is automatically registered
as a responder — its return value is the reply.

| Method | Behavior |
|--------|----------|
| `emit()` | Fan-out: spawns all subscribers as Tokio tasks, returns immediately |
| `request()` | Point-to-point: exactly one responder replies; awaits that reply with a timeout |

### 5. Integration in a service

Typically, `LocalEventBus` is injected into services:

```rust
#[derive(Clone)]
pub struct UserService {
    users: Arc<RwLock<Vec<User>>>,
    event_bus: LocalEventBus,
}

impl UserService {
    pub fn new(event_bus: LocalEventBus) -> Self {
        Self {
            users: Arc::new(RwLock::new(vec![/* ... */])),
            event_bus,
        }
    }

    pub async fn create(&self, name: String, email: String) -> User {
        let user = {
            let mut users = self.users.write().await;
            let id = users.len() as u64 + 1;
            let user = User { id, name, email };
            users.push(user.clone());
            user
        }; // Lock released here

        // Emit the event after releasing the lock
        self.event_bus
            .emit(UserCreatedEvent {
                user_id: user.id,
                name: user.name.clone(),
                email: user.email.clone(),
            })
            .await;

        user
    }
}
```

### 6. Share the bus as a bean

The event bus is provided as a bean once, then injected by type — no `FromRef` impl and no hand-written state struct:

```rust
let event_bus = LocalEventBus::new();

let app = AppBuilder::new()
    .provide(event_bus)             // the bus becomes a bean, resolved by type
    .register::<UserService>();     // UserService takes LocalEventBus via #[inject]
```

Any controller, service, or consumer receives it through a `#[inject]` field:

```rust
#[controller(path = "/users")]
pub struct UserController {
    #[inject] user_service: UserService,
    #[inject] event_bus: LocalEventBus,
}
```

### 7. Register event subscribers

`#[consumer]`-style subscribers are registered **after** `.build_state().await` with `.register_subscriber::<S>()`. The subscriber type `S` must itself be a bean (provided or registered), because it is resolved from the graph by type — never name-matched:

```rust
app.build_state()
    .await
    .register_subscriber::<UserEventConsumer>()
    .register_controller::<UserController>()
    .serve("0.0.0.0:3000")
    .await
    .unwrap();
```

## Isolation by type

Events are completely isolated by `TypeId`. Emitting an `OtherEvent` does not trigger `UserCreatedEvent` handlers:

```rust
struct OtherEvent;

bus.subscribe(|_: Arc<UserCreatedEvent>| async { println!("user!"); }).await;
bus.emit(OtherEvent).await;
// → nothing happens, the UserCreatedEvent handler is not called
```

## EventBus↔SSE bridge

`SseBridgeExt::bridge_sse::<Bus, E>()` (in the prelude) forwards every event
of type `E` emitted on the bus into a provided `SseTopic<E>` (see feature 15,
"Typed Broadcast Topics"), giving real-time SSE fan-out with zero liaison
code:

```rust
b.provide(LocalEventBus::new())
    .provide(SseTopic::<UserCreatedEvent>::new(64).with_event_name("user_created"))
    .build_state()
    .await
    .bridge_sse::<LocalEventBus, UserCreatedEvent>()
```

Emit on the bus anywhere; SSE clients subscribed to the topic's `#[sse]`
endpoint receive the JSON-serialized event. With a distributed backend
(Kafka, RabbitMQ, Pulsar, Iggy), the bridge fans out across instances. The
manual entry point is `r2e_events::sse_bridge::bridge_event_to_sse(&bus, topic)`.

## Validation criteria

Start the application and create a user:

```bash
curl -X POST http://localhost:3000/users \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"name":"Alice","email":"alice@example.com"}'
```

In the server logs:

```
INFO user_id=3 name="Alice" email="alice@example.com" "User created event received"
```
