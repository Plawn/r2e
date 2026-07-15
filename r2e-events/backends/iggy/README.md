# r2e-events-iggy

Apache Iggy event bus backend for R2E — persistent distributed event streaming.

## Overview

Provides `IggyEventBus`, a distributed implementation of the `EventBus` trait backed by [Apache Iggy](https://iggy.apache.org/). Messages are serialized as JSON and delivered with at-least-once semantics.

## Usage

```toml
[dependencies]
r2e-events-iggy = { version = "0.1" }
```

```rust
use r2e_events_iggy::prelude::*;

let config = IggyConfig::builder()
    .address("127.0.0.1:8090")
    .stream_name("my-app")
    .build();

let bus = IggyEventBus::builder(config)
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
