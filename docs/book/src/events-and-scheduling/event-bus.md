# Event Bus

R2E provides an in-process typed pub/sub event bus for decoupling components. Events are dispatched by `TypeId` — no string-based routing, no downcasting, fully type-safe at compile time.

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
use r2e::r2e_events::EventBus;

// Default: 1024 concurrent handlers max
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

The `EventBus` is `Clone` — all clones share the same subscriber list.

## Subscribing to events

Subscribers receive events wrapped in `Arc<E>` (shared across all handlers for the same emission):

```rust
event_bus.subscribe(|event: Arc<UserCreatedEvent>| async move {
    tracing::info!(user_id = event.user_id, "User created: {}", event.name);
}).await;
```

Multiple subscribers can listen to the same event type. They all run concurrently:

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

Handlers are spawned as concurrent Tokio tasks. `emit()` returns once all handlers have been **spawned** (not completed).

### Wait for completion

```rust
self.event_bus.emit_and_wait(UserCreatedEvent {
    user_id: user.id,
    name: user.name.clone(),
    email: user.email.clone(),
}).await;
```

`emit_and_wait()` blocks until all handlers **complete**. Use this when downstream processing must finish before responding (e.g., when the response depends on a side-effect).

### Comparison

| Method | Spawns handlers | Returns when | Use case |
|--------|----------------|--------------|----------|
| `emit()` | Concurrent tasks | All handlers spawned | Notifications, analytics, logging |
| `emit_and_wait()` | Concurrent tasks | All handlers completed | Side-effects the caller depends on |

Both methods respect the [concurrency limit](#concurrency-and-backpressure).

## Concurrency and backpressure

By default, `EventBus::new()` limits concurrently executing handlers to **1024** (the value of `DEFAULT_MAX_CONCURRENCY`). When the limit is reached, `emit()` blocks until a handler slot becomes available. This prevents unbounded memory growth under heavy load.

### Custom concurrency limit

```rust
// Allow at most 50 concurrent handlers
let bus = EventBus::with_concurrency(50);
```

Choose a limit based on what your handlers do:

| Handler type | Suggested limit | Rationale |
|-------------|----------------|-----------|
| CPU-bound (serialization, hashing) | Low (10–50) | Avoids starving the Tokio runtime |
| I/O-bound (HTTP calls, DB writes) | Medium (100–500) | Limited by downstream capacity |
| Lightweight (logging, counters) | High (1000+) | Default is usually fine |

### Unbounded mode

```rust
let bus = EventBus::unbounded();
```

Disables backpressure entirely. Every handler is spawned immediately regardless of load. Use with caution — if events are emitted faster than handlers can process them, memory usage grows without bound.

## Type isolation

Each event type has its own subscriber list, keyed by `TypeId`. Emitting an `OrderPlacedEvent` never triggers handlers subscribed to `UserCreatedEvent`:

```rust
bus.subscribe(|_: Arc<UserCreatedEvent>| async { println!("user!"); }).await;
bus.emit(OrderPlacedEvent { order_id: 1, total: 99.0 }).await;
// Nothing happens — different type
```

## Panic isolation

If a handler panics, the panic is caught by the Tokio task. Other handlers for the same event continue running, and the bus remains operational. With `emit_and_wait()`, panicked tasks are silently ignored (the `JoinHandle` error is discarded).

This means a single misbehaving handler cannot bring down the event bus.

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

## API reference

| Constructor | Description |
|-------------|-------------|
| `EventBus::new()` | Default concurrency limit (1024) |
| `EventBus::with_concurrency(n)` | Custom concurrency limit |
| `EventBus::unbounded()` | No concurrency limit (legacy) |

| Method | Signature | Description |
|--------|-----------|-------------|
| `subscribe` | `async fn subscribe<E, F, Fut>(&self, handler: F)` | Register a handler for event type `E` |
| `emit` | `async fn emit<E>(&self, event: E)` | Fire-and-forget: spawn handlers, return immediately |
| `emit_and_wait` | `async fn emit_and_wait<E>(&self, event: E)` | Spawn handlers, wait for all to complete |
| `concurrency_limit` | `fn concurrency_limit() -> Option<usize>` | Current limit, or `None` if unbounded |

**Type constraints:**
- Event `E`: `Send + Sync + 'static`
- Handler `F`: `Fn(Arc<E>) -> Fut + Send + Sync + 'static`
- Future `Fut`: `Future<Output = ()> + Send + 'static`

## Next steps

- [Declarative Consumers](./consumers.md) — use `#[consumer]` for cleaner event handling
- [Scheduling](./scheduling.md) — run background tasks on a timer
