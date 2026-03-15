use std::sync::Arc;
use std::time::Duration;

use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
use rdkafka::client::DefaultClientContext;
use rdkafka::producer::FutureProducer;

use r2e_events::backend::{BackendState, TopicRegistry};
use r2e_events::EventBusError;

use crate::bus::KafkaEventBus;
use crate::config::KafkaConfig;
use crate::error::map_kafka_error;
use crate::inner::KafkaInner;

/// Builder for [`KafkaEventBus`].
///
/// # Example
///
/// ```ignore
/// let bus = KafkaEventBus::builder(config)
///     .topic::<UserCreated>("user-created")
///     .topic::<OrderPlaced>("order-placed")
///     .connect()
///     .await?;
/// ```
pub struct KafkaEventBusBuilder {
    config: KafkaConfig,
    topic_registry: TopicRegistry,
}

impl KafkaEventBusBuilder {
    pub(crate) fn new(config: KafkaConfig) -> Self {
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

    /// Connect to the Kafka cluster and return a ready-to-use [`KafkaEventBus`].
    pub async fn connect(self) -> Result<KafkaEventBus, EventBusError> {
        let producer: FutureProducer = self
            .config
            .to_producer_client_config()
            .create()
            .map_err(map_kafka_error)?;

        let inner = KafkaInner {
            config: self.config,
            producer,
            state: Arc::new(BackendState::new(self.topic_registry)),
        };

        Ok(KafkaEventBus {
            inner: Arc::new(inner),
        })
    }
}

/// Ensure a topic exists in Kafka using the AdminClient (best-effort, idempotent).
pub(crate) async fn ensure_topic_exists(
    config: &KafkaConfig,
    topic_name: &str,
) -> Result<(), EventBusError> {
    let admin_client: AdminClient<DefaultClientContext> = config
        .to_admin_client_config()
        .create()
        .map_err(map_kafka_error)?;

    let new_topic = NewTopic::new(
        topic_name,
        config.default_partitions,
        TopicReplication::Fixed(config.default_replication_factor),
    );

    let opts = AdminOptions::new().operation_timeout(Some(Duration::from_secs(5)));

    match admin_client.create_topics(&[new_topic], &opts).await {
        Ok(results) => {
            for result in results {
                match result {
                    Ok(_) => {
                        tracing::info!(topic = %topic_name, "created Kafka topic");
                    }
                    Err((_, rdkafka::types::RDKafkaErrorCode::TopicAlreadyExists)) => {
                        // Topic already exists — that's fine
                    }
                    Err((topic, code)) => {
                        tracing::warn!(
                            topic = %topic,
                            error = ?code,
                            "failed to create Kafka topic (may already exist)"
                        );
                    }
                }
            }
            Ok(())
        }
        Err(e) => {
            tracing::warn!(topic = %topic_name, "admin create_topics error: {e}");
            // Non-fatal — topic might already exist or auto.create.topics.enable is on
            Ok(())
        }
    }
}
