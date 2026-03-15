//! RabbitMQ (AMQP 0-9-1) event bus backend for R2E.
//!
//! Provides [`RabbitMqEventBus`] — a distributed implementation of the
//! [`EventBus`](r2e_events::EventBus) trait backed by
//! [RabbitMQ](https://www.rabbitmq.com/) via the [`lapin`] AMQP 0-9-1 client.
//!
//! # Architecture
//!
//! - **Emit path:** Serialize event → publish JSON message to a topic exchange
//!   with routing key = topic name
//! - **Consume path:** Per-topic queue bound to exchange → `basic_consume` stream
//!   → deserialize → dispatch to local handlers → ack/nack
//!
//! # AMQP Model Mapping
//!
//! | R2E concept        | RabbitMQ concept                              |
//! |--------------------|-----------------------------------------------|
//! | Event bus          | Topic exchange                                |
//! | Event type         | Routing key (= topic name)                    |
//! | Consumer group     | Queue named `{consumer_group}.{topic_name}`   |
//! | Competing consumer | Multiple instances consuming the same queue   |
//! | Metadata           | AMQP message headers                          |
//!
//! # Quick Start
//!
//! ```ignore
//! use r2e_events_rabbitmq::prelude::*;
//!
//! let config = RabbitMqConfig::builder()
//!     .uri("amqp://guest:guest@localhost:5672/%2f")
//!     .exchange("my-events")
//!     .build();
//!
//! let bus = RabbitMqEventBus::builder(config)
//!     .topic::<UserCreated>("user-created")
//!     .connect()
//!     .await?;
//!
//! bus.subscribe(|env: EventEnvelope<UserCreated>| async move {
//!     println!("user created: {:?}", env.event);
//!     HandlerResult::Ack
//! }).await?;
//!
//! bus.emit(UserCreated { id: 1, name: "Alice".into() }).await?;
//! ```
//!
//! # Limitations
//!
//! - `emit_and_wait` publishes to RabbitMQ AND waits for **local** handlers only.
//! - RabbitMQ has no native partitioning. `partition_key` is stored as a header only.

mod builder;
mod bus;
mod config;
mod error;
mod inner;

pub use builder::RabbitMqEventBusBuilder;
pub use bus::RabbitMqEventBus;
pub use config::{RabbitMqConfig, RabbitMqConfigBuilder};
pub use error::map_lapin_error;
pub use r2e_events::backend::sanitize_topic_name;

pub mod prelude {
    //! Re-exports of the most commonly used types.
    pub use crate::{RabbitMqConfig, RabbitMqEventBus};
    pub use r2e_events::prelude::*;
}
