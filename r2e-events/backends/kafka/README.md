# r2e-events-kafka

Apache Kafka event bus backend for R2E — distributed event streaming.

## Overview

Provides `KafkaEventBus`, a distributed implementation of the `EventBus` trait backed by [Apache Kafka](https://kafka.apache.org/) via `rdkafka`. Messages are serialized as JSON and delivered with at-least-once semantics.

## Usage

```toml
[dependencies]
r2e-events-kafka = { version = "0.1" }
```

```rust
use r2e_events_kafka::prelude::*;

let config = KafkaConfig::builder()
    .bootstrap_servers("localhost:9092")
    .group_id("my-app")
    .build();

let bus = KafkaEventBus::builder(config)
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

At-least-once. Messages are committed only after all local handlers have resolved. Handlers must be idempotent.

## License

Apache-2.0
