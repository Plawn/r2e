# r2e-events

In-process typed event bus for R2E — publish/subscribe with async handlers and backpressure support.

## Overview

Provides a lightweight, typed event bus where events are dispatched by `TypeId`. Subscribers receive `Arc<E>` and handlers run as concurrent Tokio tasks. A semaphore-based backpressure mechanism limits concurrent handlers (default: 1024).

## Usage

Via the facade crate (enabled by default):

```toml
[dependencies]
r2e = "0.1"  # events is a default feature
```

## API

```rust
use r2e::r2e_events::EventBus;
use std::sync::Arc;

// Default concurrency limit (1024)
let bus = EventBus::new();

// Custom concurrency limit
let bus = EventBus::with_concurrency(50);

// No limit (legacy behavior)
let bus = EventBus::unbounded();

// Subscribe to an event type
bus.subscribe(|event: Arc<UserCreated>| async move {
    println!("User created: {}", event.name);
}).await;

// Fire-and-forget — handlers spawned as concurrent tasks
bus.emit(UserCreated { name: "Alice".into() }).await;

// Wait for all handlers to complete
bus.emit_and_wait(UserCreated { name: "Bob".into() }).await;
```

## Declarative consumers

Use `#[consumer]` in a `#[routes]` impl block for automatic event subscription:

```rust
#[derive(Controller)]
#[controller(path = "/notifications", state = AppState)]
pub struct NotificationController {
    #[inject] bus: EventBus,
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

Consumers are registered automatically during `register_controller()`. The controller must not have `#[inject(identity)]` struct fields (requires `StatefulConstruct`).

## Key properties

- **Type-safe dispatch** — events routed by `TypeId`, no string-based routing
- **Backpressure** — semaphore limits concurrent handlers, `emit()` blocks when full
- **Panic isolation** — a panicking handler doesn't affect other handlers or the bus
- **Clone-friendly** — all clones share the same subscriber list

## License

Apache-2.0
