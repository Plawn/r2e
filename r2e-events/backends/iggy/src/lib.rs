//! Apache Iggy event bus backend for R2E.
//!
//! Provides [`IggyEventBus`] — a distributed implementation of the
//! [`EventBus`](r2e_events::EventBus) trait backed by
//! [Apache Iggy](https://iggy.apache.org/), a persistent message streaming platform.
//!
//! # Architecture
//!
//! - **Emit path:** Serialize event → publish JSON message to Iggy topic
//! - **Consume path:** Background poller per topic → deserialize → dispatch to local handlers
//!
//! # Quick Start
//!
//! ```ignore
//! use r2e_events_iggy::prelude::*;
//!
//! let config = IggyConfig::builder()
//!     .address("127.0.0.1:8090")
//!     .stream_name("my-app")
//!     .build();
//!
//! let bus = IggyEventBus::builder(config)
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
//! - `emit_and_wait` publishes to Iggy AND waits for **local** handlers only.
//! - One event type per topic (determined by first `subscribe` call).

mod builder;
mod bus;
mod config;
mod dispatch;
mod error;
mod inner;
mod poller;
mod topic;

pub use builder::IggyEventBusBuilder;
pub use bus::IggyEventBus;
pub use config::{IggyConfig, IggyConfigBuilder, Transport};
pub use error::map_iggy_error;
pub use r2e_events::backend::sanitize_topic_name;

pub mod prelude {
    //! Re-exports of the most commonly used types.
    pub use crate::{IggyConfig, IggyEventBus, Transport};
    pub use r2e_events::prelude::*;
}
