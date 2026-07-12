use std::sync::Arc;

use r2e_events::backend::{BackendState, TopicRegistry};
use r2e_events::EventBusError;

use crate::bus::RabbitMqEventBus;
use crate::config::RabbitMqConfig;
use crate::inner::RabbitMqInner;

/// Builder for [`RabbitMqEventBus`].
///
/// # Example
///
/// ```ignore
/// let bus = RabbitMqEventBus::builder(config)
///     .topic::<UserCreated>("user-created")
///     .topic::<OrderPlaced>("order-placed")
///     .connect()
///     .await?;
/// ```
pub struct RabbitMqEventBusBuilder {
    config: RabbitMqConfig,
    topic_registry: TopicRegistry,
}

impl RabbitMqEventBusBuilder {
    pub(crate) fn new(config: RabbitMqConfig) -> Self {
        Self {
            config,
            topic_registry: TopicRegistry::default(),
        }
    }

    /// Register an explicit topic name for event type `E`.
    pub fn topic<E: 'static>(mut self, name: impl Into<String>) -> Self {
        self.topic_registry.register::<E>(name);
        self
    }

    /// Register an event type using its [`Event::topic()`] name.
    pub fn register_event<E: r2e_events::Event + 'static>(self) -> Self {
        self.topic::<E>(E::topic())
    }

    /// Connect to the RabbitMQ broker and return a ready-to-use [`RabbitMqEventBus`].
    pub async fn connect(self) -> Result<RabbitMqEventBus, EventBusError> {
        // Open the connection. It is retained on the inner so channels can be
        // transparently recreated after a broker blip (publisher + per-consumer
        // channels are created lazily from it).
        let connection = RabbitMqInner::connect(&self.config).await?;

        tracing::info!(uri = %self.config.uri, "connected to RabbitMQ");

        let inner = RabbitMqInner::new(
            self.config,
            connection,
            Arc::new(BackendState::new(self.topic_registry)),
        );

        // Prime the publisher channel now so exchange declaration and other
        // configuration errors surface at `connect()` time rather than on the
        // first `emit`.
        inner.publisher_channel().await?;

        Ok(RabbitMqEventBus {
            inner: Arc::new(inner),
        })
    }
}
