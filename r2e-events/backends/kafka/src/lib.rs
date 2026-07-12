//! Apache Kafka event bus backend for R2E.
//!
//! Provides [`KafkaEventBus`] — a distributed implementation of the
//! [`EventBus`](r2e_events::EventBus) trait backed by
//! [Apache Kafka](https://kafka.apache.org/), the industry-standard distributed
//! event streaming platform.
//!
//! # Architecture
//!
//! - **Emit path:** Serialize event → publish JSON message to Kafka topic
//!   (using `FutureProducer`)
//! - **Consume path:** `StreamConsumer` per topic → deserialize → dispatch to
//!   local handlers
//!
//! # Quick Start
//!
//! ```ignore
//! use r2e_events_kafka::prelude::*;
//!
//! let config = KafkaConfig::builder()
//!     .bootstrap_servers("localhost:9092")
//!     .group_id("my-app")
//!     .build();
//!
//! let bus = KafkaEventBus::builder(config)
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
//! - `emit_and_wait` publishes to Kafka AND waits for **local** handlers only.
//! - One event type per topic (determined by first `subscribe` call).

mod builder;
mod bus;
mod config;
mod error;
mod inner;

pub use builder::KafkaEventBusBuilder;
pub use bus::KafkaEventBus;
pub use config::{Acks, Compression, KafkaConfig, KafkaConfigBuilder, SecurityProtocol};
pub use error::map_kafka_error;

pub mod prelude {
    //! Re-exports of the most commonly used types.
    pub use crate::{KafkaConfig, KafkaEventBus};
    pub use r2e_events::prelude::*;
}
