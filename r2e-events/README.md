# r2e-events

Typed event bus for R2E — fan-out publish/subscribe and point-to-point request-reply with async handlers and backpressure support.

## Overview

`EventBus` is a trait; `LocalEventBus` is the built-in in-process implementation where events are dispatched by `TypeId`. Subscribers receive an `EventEnvelope<E>` (the event payload plus metadata) and handlers run as concurrent Tokio tasks. A semaphore-based backpressure mechanism limits concurrent handlers (default: 1024).

Distributed backends (Iggy, Kafka, Pulsar, RabbitMQ) live under `backends/` and share the utilities in the `backend` module.

## Usage

Via the facade crate (enabled by default):

```toml
[dependencies]
r2e = "0.1"  # events is a default feature
```

## API

```rust
use r2e::r2e_events::prelude::*;

// Default concurrency limit (1024)
let bus = LocalEventBus::new();

// Custom concurrency limit
let bus = LocalEventBus::with_concurrency(50);

// No limit (legacy behavior)
let bus = LocalEventBus::unbounded();

// Subscribe to an event type — the handler receives an EventEnvelope<E>
// and returns a HandlerResult (a `()` return converts to HandlerResult::Ack).
bus.subscribe(|env: EventEnvelope<UserCreated>| async move {
    println!("User created: {}", env.event.name);
    HandlerResult::Ack
}).await?;

// Fan-out publish — handlers spawned as concurrent tasks
bus.emit(UserCreated { name: "Alice".into() }).await?;

// Drain all in-flight handlers (LocalEventBus only; useful in tests)
bus.wait_idle().await;
```

### Request-reply

Point-to-point request-reply (Vert.x `request` semantics): exactly one
responder replies and the requester awaits it with a timeout.

```rust
// Register the single responder for a request type
bus.respond(|env: EventEnvelope<GetUser>| async move {
    Ok::<_, String>(User { id: env.event.id, name: "Alice".into() })
}).await?;

// Send a request and await the reply
let user: User = bus.request(GetUser { id: 1 }).await?;
```

## Declarative consumers

Use `#[consumer]` in a `#[routes]` impl block for automatic event subscription:

```rust
#[controller(path = "/notifications")]
pub struct NotificationController {
    #[inject] bus: LocalEventBus,
    #[inject] mailer: MailService,
}

#[routes]
impl NotificationController {
    #[consumer(bus = "bus")]
    async fn on_user_created(&self, event: Arc<UserCreated>) {
        self.mailer.send_welcome(&event.email).await;
    }
}
```

Consumers are registered automatically during `register_controller()`. They run on the controller core, which always implements `ContextConstruct` (built from the resolved `BeanContext` by type), so they work regardless of any `#[inject(identity)]` fields.

Standalone event subscribers (beans that are not controllers) are auto-collected:
`#[bean]` emits an `after_register` hook, so `.register::<S>()` alone is enough —
`build_state()` queues the subscription (resolved from the graph by type) and it
runs at server startup, at the same point controller consumers subscribe.

## Key properties

- **Type-safe dispatch** — events routed by `TypeId`, no string-based routing
- **Backpressure** — semaphore limits concurrent handlers, `emit()` blocks when full
- **Panic isolation** — a panicking handler doesn't affect other handlers or the bus
- **Clone-friendly** — all clones share the same subscriber list

## License

Apache-2.0
