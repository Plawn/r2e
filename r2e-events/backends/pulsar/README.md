# r2e-events-pulsar

Apache Pulsar event bus backend for R2E — distributed event streaming.

## Overview

Provides `PulsarEventBus`, a distributed implementation of the `EventBus` trait backed by [Apache Pulsar](https://pulsar.apache.org/). Messages are serialized as JSON and delivered with at-least-once semantics.

## Usage

```toml
[dependencies]
r2e-events-pulsar = { version = "0.1" }
```

```rust
use r2e_events_pulsar::prelude::*;

let config = PulsarConfig::builder()
    .service_url("pulsar://localhost:6650")
    .subscription("my-app")
    .build();

let bus = PulsarEventBus::builder(config)
    .topic::<UserCreated>("user-created")
    .connect()
    .await?;

bus.subscribe(|env: EventEnvelope<UserCreated>| async move {
    println!("user created: {:?}", env.event);
    HandlerResult::Ack
}).await?;

bus.emit(UserCreated { id: 1, name: "Alice".into() }).await?;
```

## Messaging models

- **`emit`** — fan-out publish/subscribe
- **`request` / `respond`** — point-to-point request-reply with timeout

## Delivery semantics

At-least-once. Messages are acked only after all local handlers have resolved. Handlers must be idempotent.

## License

Apache-2.0
