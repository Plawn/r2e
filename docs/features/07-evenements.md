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

// Emit and wait for all handlers to complete
event_bus.emit_and_wait(UserCreatedEvent {
    user_id: 42,
    name: "Alice".into(),
    email: "alice@example.com".into(),
}).await;
```

### Difference between `emit` and `emit_and_wait`

| Method | Behavior |
|--------|----------|
| `emit()` | Spawns handlers as Tokio tasks, returns immediately |
| `emit_and_wait()` | Spawns handlers, waits for all of them to complete |

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

### 6. Share the bus via application state

```rust
#[derive(Clone)]
pub struct Services {
    pub user_service: UserService,
    pub event_bus: LocalEventBus,
    // ...
}

impl axum::extract::FromRef<Services> for LocalEventBus {
    fn from_ref(state: &Services) -> Self {
        state.event_bus.clone()
    }
}
```

## Isolation by type

Events are completely isolated by `TypeId`. Emitting an `OtherEvent` does not trigger `UserCreatedEvent` handlers:

```rust
struct OtherEvent;

bus.subscribe(|_: Arc<UserCreatedEvent>| async { println!("user!"); }).await;
bus.emit(OtherEvent).await;
// → nothing happens, the UserCreatedEvent handler is not called
```

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
