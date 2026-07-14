use std::sync::Arc;
use std::time::Duration;

use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
use rdkafka::client::DefaultClientContext;
use rdkafka::producer::FutureProducer;

use r2e_events::backend::{instance_id, reply_topic, BackendState, PendingRequests, TopicRegistry};
use r2e_events::{DlqPublisher, EventBusError};

use crate::bus::KafkaEventBus;
use crate::config::KafkaConfig;
use crate::error::map_kafka_error;
use crate::inner::KafkaInner;

/// Short retention for instance-private reply topics. These topics are unique
/// per bus lifetime and would otherwise accumulate after restarts.
pub(crate) const REPLY_TOPIC_RETENTION_MS: &str = "300000";

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

        // Mint one instance nonce per bus and derive its (constant) reply topic
        // once — both the reply topic and the reply consumer group embed this id
        // so two bus instances sharing a config in one process stay disjoint.
        let instance_id = instance_id();
        let reply_topic = reply_topic(&self.config.group_id, instance_id);

        let inner = Arc::new_cyclic(|weak: &std::sync::Weak<KafkaInner>| {
            let weak = weak.clone();
            let dlq: DlqPublisher = Arc::new(move |topic, payload, metadata| {
                let weak = weak.clone();
                Box::pin(async move {
                    let inner = weak.upgrade().ok_or(EventBusError::Shutdown)?;
                    KafkaEventBus { inner }
                        .publish(&topic, payload, &metadata)
                        .await
                })
            });
            KafkaInner {
                config: self.config,
                producer,
                state: Arc::new(BackendState::with_dlq_publisher(
                    self.topic_registry,
                    Some(dlq),
                )),
                pending: Arc::new(PendingRequests::new()),
                reply_consumer: tokio::sync::OnceCell::new(),
                responder_cancels: std::sync::Mutex::new(std::collections::HashMap::new()),
                request_cancel: tokio_util::sync::CancellationToken::new(),
                instance_id,
                reply_topic,
            }
        });

        Ok(KafkaEventBus { inner })
    }
}

/// Ensure a topic exists in Kafka using the AdminClient (best-effort, idempotent).
pub(crate) async fn ensure_topic_exists(
    config: &KafkaConfig,
    topic_name: &str,
    retention_ms: Option<&str>,
) -> Result<(), EventBusError> {
    let admin_client: AdminClient<DefaultClientContext> = config
        .to_admin_client_config()
        .create()
        .map_err(map_kafka_error)?;

    let new_topic = topic_definition(config, topic_name, retention_ms);

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

fn topic_definition<'a>(
    config: &KafkaConfig,
    topic_name: &'a str,
    retention_ms: Option<&'a str>,
) -> NewTopic<'a> {
    let mut topic = NewTopic::new(
        topic_name,
        config.default_partitions,
        TopicReplication::Fixed(config.default_replication_factor),
    );
    if let Some(retention_ms) = retention_ms {
        topic = topic.set("retention.ms", retention_ms);
    }
    topic
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_topic_retention_is_short() {
        let config = KafkaConfig::default();
        let topic = topic_definition(
            &config,
            "r2e.replies.instance",
            Some(REPLY_TOPIC_RETENTION_MS),
        );

        assert_eq!(topic.config, vec![("retention.ms", "300000")]);
    }
}
