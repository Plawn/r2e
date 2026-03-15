use std::sync::Arc;

use lapin::{
    options::ExchangeDeclareOptions,
    types::FieldTable,
    Connection, ConnectionProperties, ExchangeKind,
};

use r2e_events::backend::{BackendState, TopicRegistry};
use r2e_events::EventBusError;

use crate::bus::RabbitMqEventBus;
use crate::config::RabbitMqConfig;
use crate::error::map_lapin_error;
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
        // Build connection properties
        let mut conn_props = ConnectionProperties::default()
            .with_connection_name(
                self.config
                    .connection_name
                    .clone()
                    .unwrap_or_else(|| "r2e-events-rabbitmq".into())
                    .into(),
            );

        // Set heartbeat via executor
        // lapin ConnectionProperties uses the tokio executor by default
        conn_props = conn_props.with_executor(tokio_executor_trait::Tokio::current())
            .with_reactor(tokio_reactor_trait::Tokio);

        // Connect to RabbitMQ
        let connection = Connection::connect(&self.config.uri, conn_props)
            .await
            .map_err(map_lapin_error)?;

        tracing::info!(uri = %self.config.uri, "connected to RabbitMQ");

        // Create a channel
        let channel = connection.create_channel().await.map_err(map_lapin_error)?;

        // Set QoS (prefetch count)
        channel
            .basic_qos(
                self.config.prefetch_count,
                lapin::options::BasicQosOptions::default(),
            )
            .await
            .map_err(map_lapin_error)?;

        // Declare the topic exchange if auto_create is on
        if self.config.auto_create {
            channel
                .exchange_declare(
                    &self.config.exchange,
                    ExchangeKind::Topic,
                    ExchangeDeclareOptions {
                        durable: self.config.durable,
                        ..ExchangeDeclareOptions::default()
                    },
                    FieldTable::default(),
                )
                .await
                .map_err(map_lapin_error)?;

            tracing::info!(exchange = %self.config.exchange, "declared topic exchange");
        }

        let inner = RabbitMqInner {
            config: self.config,
            channel,
            state: Arc::new(BackendState::new(self.topic_registry)),
        };

        Ok(RabbitMqEventBus {
            inner: Arc::new(inner),
        })
    }
}
