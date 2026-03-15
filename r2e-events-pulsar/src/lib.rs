//! Apache Pulsar event bus backend for R2E.
//!
//! Provides [`PulsarEventBus`] — a distributed implementation of the
//! [`EventBus`](r2e_events::EventBus) trait backed by
//! [Apache Pulsar](https://pulsar.apache.org/), a distributed messaging and
//! streaming platform.
//!
//! # Architecture
//!
//! - **Emit path:** Serialize event -> publish JSON message to Pulsar topic
//! - **Consume path:** Background consumer per topic -> deserialize -> dispatch to local handlers
//!
//! # Quick Start
//!
//! ```ignore
//! use r2e_events_pulsar::prelude::*;
//!
//! let config = PulsarConfig::builder()
//!     .service_url("pulsar://localhost:6650")
//!     .subscription("my-app")
//!     .build();
//!
//! let bus = PulsarEventBus::builder(config)
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
//! - `emit_and_wait` publishes to Pulsar AND waits for **local** handlers only.
//! - One event type per topic (determined by first `subscribe` call).

mod builder;
mod bus;
mod config;
mod error;
mod inner;

pub use builder::PulsarEventBusBuilder;
pub use bus::PulsarEventBus;
pub use config::{PulsarConfig, PulsarConfigBuilder, SubscriptionType};
pub use error::map_pulsar_error;
pub use r2e_events::backend::sanitize_topic_name;

pub mod prelude {
    //! Re-exports of the most commonly used types.
    pub use crate::{PulsarConfig, PulsarEventBus, SubscriptionType};
    pub use r2e_events::prelude::*;
}
