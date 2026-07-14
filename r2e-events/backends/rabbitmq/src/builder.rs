use std::sync::Arc;

use r2e_events::backend::{BackendState, TopicRegistry};
use r2e_events::{DlqPublisher, EventBusError};

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

        let inner = Arc::new_cyclic(|weak: &std::sync::Weak<RabbitMqInner>| {
            let weak = weak.clone();
            let dlq: DlqPublisher = Arc::new(move |topic, payload, metadata| {
                let weak = weak.clone();
                Box::pin(async move {
                    let inner = weak.upgrade().ok_or(EventBusError::Shutdown)?;
                    // A topic-exchange publish with no bound queue is accepted
                    // but discarded by RabbitMQ. Ensure the current group's
                    // durable DLQ queue/binding exists before confirming the
                    // source message as parked.
                    let setup = inner.new_consumer_channel().await?;
                    inner.ensure_queue(&setup, &topic).await?;
                    RabbitMqEventBus { inner }
                        .publish(&topic, payload, &metadata)
                        .await
                })
            });
            RabbitMqInner::new(
                self.config,
                connection,
                Arc::new(BackendState::with_dlq_publisher(
                    self.topic_registry,
                    Some(dlq),
                )),
            )
        });

        // Prime the publisher channel now so exchange declaration and other
        // configuration errors surface at `connect()` time rather than on the
        // first `emit`.
        inner.publisher_channel().await?;

        Ok(RabbitMqEventBus { inner })
    }
}
