# Event Bus

R2E provides an in-process typed pub/sub event bus for decoupling components.

## Setup

Enable the events feature:

```toml
r2e = { version = "0.1", features = ["events"] }
```

## Defining events

Events are plain Rust types. No trait implementation needed — just `Send + Sync + 'static`:

```rust
#[derive(Debug, Clone)]
pub struct UserCreatedEvent {
    pub user_id: u64,
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone)]
pub struct OrderPlacedEvent {
    pub order_id: u64,
    pub total: f64,
}
```

## Creating the EventBus

```rust
let event_bus = EventBus::new();
```

Add it to your state:

```rust
#[derive(Clone, BeanState)]
pub struct AppState {
    pub event_bus: EventBus,
    // ...
}
```

## Subscribing to events

Subscribers receive events wrapped in `Arc<E>`:

```rust
event_bus.subscribe(|event: Arc<UserCreatedEvent>| async move {
    tracing::info!(user_id = event.user_id, "User created: {}", event.name);
}).await;
```

Multiple subscribers can listen to the same event type:

```rust
// Send welcome email
event_bus.subscribe(|event: Arc<UserCreatedEvent>| async move {
    send_welcome_email(&event.email).await;
}).await;

// Update analytics
event_bus.subscribe(|event: Arc<UserCreatedEvent>| async move {
    analytics.track_signup(event.user_id).await;
}).await;
```

## Emitting events

### Fire-and-forget

```rust
self.event_bus.emit(UserCreatedEvent {
    user_id: user.id,
    name: user.name.clone(),
    email: user.email.clone(),
}).await;
```

Handlers are spawned as concurrent tasks — `emit()` returns immediately.

### Wait for completion

```rust
self.event_bus.emit_and_wait(UserCreatedEvent {
    user_id: user.id,
    name: user.name.clone(),
    email: user.email.clone(),
}).await;
```

`emit_and_wait()` blocks until all handlers complete. Use this when downstream processing must finish before responding.

## Type safety

Events are dispatched by `TypeId`. Each event type has its own set of subscribers. There's no downcasting or string-based routing — everything is type-safe at compile time.

## In services

```rust
#[derive(Clone)]
pub struct UserService {
    event_bus: EventBus,
    // ...
}

#[bean]
impl UserService {
    pub fn new(event_bus: EventBus) -> Self {
        Self { event_bus }
    }

    pub async fn create(&self, name: String, email: String) -> User {
        let user = /* create user */;

        // Notify interested parties
        self.event_bus.emit(UserCreatedEvent {
            user_id: user.id,
            name: user.name.clone(),
            email: user.email.clone(),
        }).await;

        user
    }
}
```

## Next steps

- [Declarative Consumers](./consumers.md) — use `#[consumer]` for cleaner event handling
