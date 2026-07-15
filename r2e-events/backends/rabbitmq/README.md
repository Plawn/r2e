# r2e-events-rabbitmq

RabbitMQ (AMQP 0-9-1) event bus backend for R2E — durable message queuing.

## Overview

Provides `RabbitMqEventBus`, a distributed implementation of the `EventBus` trait backed by [RabbitMQ](https://www.rabbitmq.com/) via the `lapin` AMQP client. Messages are serialized as JSON and delivered with at-least-once semantics.

## AMQP model mapping

| R2E concept | RabbitMQ concept |
|-------------|------------------|
| Event bus | Topic exchange |
| Event type | Routing key (= topic name) |
| Consumer group | Queue named `{consumer_group}.{topic_name}` |
| Competing consumer | Multiple instances consuming the same queue |
| Metadata | AMQP message headers |

## Usage

```toml
[dependencies]
r2e-events-rabbitmq = { version = "0.1" }
```

```rust
use r2e_events_rabbitmq::prelude::*;

let config = RabbitMqConfig::builder()
    .uri("amqp://guest:guest@localhost:5672/%2f")
    .exchange("my-events")
    .build();

let bus = RabbitMqEventBus::builder(config)
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
