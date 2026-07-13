# Event Bus

R2E provides a pluggable event bus for decoupling components. The `EventBus` trait defines the interface; `LocalEventBus` is the default in-process implementation. Events are dispatched by `TypeId` — no string-based routing, no downcasting, fully type-safe at compile time.

Custom backends (Kafka, Redis, NATS) can implement the `EventBus` trait for remote event transport.

## Setup

Enable the events feature:

```toml
r2e = { version = "0.1", features = ["events"] }
```

## Defining events

Events must implement `Serialize + DeserializeOwned + Send + Sync + 'static`. The serde bounds are required by the `EventBus` trait for backend compatibility. `LocalEventBus` never actually serializes — zero overhead.

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UserCreatedEvent {
    pub user_id: u64,
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OrderPlacedEvent {
    pub order_id: u64,
    pub total: f64,
}
```

## Creating a LocalEventBus

```rust
use r2e::r2e_events::{EventBus, LocalEventBus};

// Default: 1024 concurrent handlers max
let event_bus = LocalEventBus::new();
```

Provide it as a bean so it can be injected by type:

```rust
let app = AppBuilder::new()
    .provide(event_bus)
    // ... other beans
    ;
```

Any controller or bean can then inject it by type:

```rust
#[controller]
pub struct MyController {
    #[inject] event_bus: LocalEventBus,
}
```

`LocalEventBus` is `Clone` — all clones share the same subscriber list.

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

## Two messaging models

R2E's bus follows the Vert.x split between two distinct interaction styles:

- **`emit` — fan-out, fire-and-forget** (Vert.x `publish`). Every subscriber receives a copy; the emitter never waits for handlers and cannot observe a reply. Use it for notifications, analytics, logging — anything where the caller does not depend on the outcome.
- **`request` — point-to-point with one reply** (Vert.x `request`). Exactly one responder handles the message and returns a value; the requester awaits that reply. Use it when you need an answer back.

## Emitting events (fan-out)

```rust
self.event_bus.emit(UserCreatedEvent {
    user_id: user.id,
    name: user.name.clone(),
    email: user.email.clone(),
}).await?;
```

Handlers are spawned as concurrent Tokio tasks. `emit()` returns once all handlers have been **spawned** (not completed) — it is fire-and-forget by design, so it never blocks on downstream work and never surfaces a handler's result. Emitting respects the [concurrency limit](#concurrency-and-backpressure).

## Request-reply (point-to-point)

When you need a value back, use `request`/`respond` instead of `emit`. One responder is registered per request type; the requester awaits its reply.

Define the request and reply as ordinary event types:

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GreetRequest {
    pub name: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GreetReply {
    pub message: String,
}
```

Register the responder with `respond`. The handler receives the request and returns `Result<Resp, String>` — the `Ok` value becomes the reply, and `Err(msg)` reaches the requester as `EventBusError::Remote(msg)`:

```rust
event_bus.respond(|req: EventEnvelope<GreetRequest>| async move {
    Ok(GreetReply { message: format!("Hello, {}!", req.event.name) })
}).await?;
```

Send a request from anywhere with the injected bus and await the reply:

```rust
let reply: GreetReply = self.event_bus
    .request(GreetRequest { name: "Alice".into() })
    .await?;
// reply.message == "Hello, Alice!"
```

### Responder via `#[consumer]`

A `#[consumer]` method with a **non-`()` return type** is registered as a responder automatically (Quarkus `@ConsumeEvent`-style — the return value IS the reply). A `-> ()` consumer stays a plain fan-out subscriber:

```rust
#[routes]
impl UserEventConsumer {
    // Plain fan-out subscriber: `-> ()`, registered via `subscribe`.
    #[consumer(bus = "event_bus")]
    async fn on_user_created(&self, event: Arc<UserCreatedEvent>) {
        tracing::info!(user_id = event.user_id, "user created");
    }

    // Responder: non-`()` return, registered via `respond`. The return
    // value is delivered to `bus.request`.
    #[consumer(bus = "event_bus")]
    async fn greet(&self, req: Arc<GreetRequest>) -> GreetReply {
        GreetReply { message: format!("Hello, {}!", req.name) }
    }
}
```

### One responder per type

**At most one responder may be registered per request type per process.** A second registration returns an error. This is deliberate: point-to-point means one reply, so there is no in-process round-robin. When you scale across instances, cross-instance load balancing comes from the broker's queue / consumer-group semantics (each request goes to one consumer), not from the bus.

### Timeouts and errors

`request` applies a **30-second default timeout** (`DEFAULT_REQUEST_TIMEOUT`). Use `request_with` and `RequestOptions` to override the timeout or attach explicit metadata:

```rust
use std::time::Duration;
use r2e::r2e_events::RequestOptions;

let reply: GreetReply = self.event_bus
    .request_with(
        GreetRequest { name: "Alice".into() },
        RequestOptions::new().with_timeout(Duration::from_secs(5)),
    )
    .await?;
```

`request` fails with an `EventBusError`:

| Variant | Meaning |
|---------|---------|
| `NoResponder` | No responder is registered for the request type. **Local bus only** — distributed backends can't see remote registrations, so an absent responder surfaces as `RequestTimeout` instead. |
| `RequestTimeout` | No reply arrived within the timeout. |
| `Remote(msg)` | The responder ran but returned `Err(msg)` (the Vert.x `ReplyException` equivalent). |

`respond` returns a `ResponderHandle`; call `unregister()` on it to remove the responder so a different one can take its place.

## Concurrency and backpressure

By default, `LocalEventBus::new()` limits concurrently executing handlers to **1024** (the value of `DEFAULT_MAX_CONCURRENCY`). When the limit is reached, `emit()` blocks until a handler slot becomes available. This prevents unbounded memory growth under heavy load.

### Custom concurrency limit

```rust
// Allow at most 50 concurrent handlers
let bus = LocalEventBus::with_concurrency(50);
```

Choose a limit based on what your handlers do:

| Handler type | Suggested limit | Rationale |
|-------------|----------------|-----------|
| CPU-bound (serialization, hashing) | Low (10–50) | Avoids starving the Tokio runtime |
| I/O-bound (HTTP calls, DB writes) | Medium (100–500) | Limited by downstream capacity |
| Lightweight (logging, counters) | High (1000+) | Default is usually fine |

### Unbounded mode

```rust
let bus = LocalEventBus::unbounded();
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

If a handler panics, the panic is caught by the Tokio task. Other handlers for the same event continue running, and the bus remains operational.

This means a single misbehaving handler cannot bring down the event bus.

## In services

```rust
#[derive(Clone)]
pub struct UserService {
    event_bus: LocalEventBus,
    // ...
}

#[bean]
impl UserService {
    pub fn new(event_bus: LocalEventBus) -> Self {
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

### `EventBus` trait

| Method | Signature | Description |
|--------|-----------|-------------|
| `subscribe` | `async fn subscribe<E, F, Fut>(&self, handler: F)` | Register a fan-out handler for event type `E` |
| `emit` | `async fn emit<E>(&self, event: E)` | Fire-and-forget fan-out: spawn handlers, return immediately |
| `request` | `async fn request<Req, Resp>(&self, req: Req) -> Result<Resp, _>` | Point-to-point: await one responder's reply (30s default timeout) |
| `request_with` | `async fn request_with<Req, Resp>(&self, req: Req, options: RequestOptions)` | `request` with explicit timeout/metadata |
| `respond` | `async fn respond<Req, Resp, F, Fut>(&self, handler: F)` | Register the responder for `Req` (one per type per process) |
| `clear` | `async fn clear(&self)` | Remove all subscribers |

**Type constraints:**
- `subscribe`: `E: DeserializeOwned + Send + Sync + 'static`
- `emit`: `E: Serialize + Send + Sync + 'static`
- `request`: `Req: Serialize + Send + Sync + 'static`, `Resp: DeserializeOwned + Send + 'static`
- `respond`: `Req: DeserializeOwned + Send + Sync + 'static`, `Resp: Serialize + Send + 'static`; handler returns `Result<Resp, String>`
- Handler `F`: `Fn(Arc<E>) -> Fut + Send + Sync + 'static`
- Future `Fut`: `Future<Output = ()> + Send + 'static`

### `LocalEventBus` (default implementation)

| Constructor | Description |
|-------------|-------------|
| `LocalEventBus::new()` | Default concurrency limit (1024) |
| `LocalEventBus::with_concurrency(n)` | Custom concurrency limit |
| `LocalEventBus::unbounded()` | No concurrency limit |
| `concurrency_limit()` | Current limit, or `None` if unbounded |

### Custom backends

Implement the `EventBus` trait for remote transport:

```rust
#[derive(Clone)]
pub struct KafkaEventBus { /* ... */ }

impl EventBus for KafkaEventBus {
    fn subscribe<E, F, Fut>(&self, handler: F) -> impl Future<Output = ()> + Send
    where E: DeserializeOwned + Send + Sync + 'static, /* ... */
    { /* consume from Kafka topic, deserialize, dispatch */ }

    fn emit<E>(&self, event: E) -> impl Future<Output = ()> + Send
    where E: Serialize + Send + Sync + 'static
    { /* serialize and produce to Kafka topic */ }

    // ...
}
```

## Next steps

- [Declarative Consumers](./consumers.md) — use `#[consumer]` for cleaner event handling
- [Scheduling](./scheduling.md) — run background tasks on a timer
