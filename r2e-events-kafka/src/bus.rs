use std::any::TypeId;
use std::future::Future;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::message::Headers;
use rdkafka::producer::{FutureRecord, Producer};
use rdkafka::Message;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use r2e_events::backend::{decode_metadata, encode_metadata, Handler};
use r2e_events::{
    EventBus, EventBusError, EventEnvelope, EventMetadata, HandlerResult, SubscriptionHandle,
};

use crate::builder::{ensure_topic_exists, KafkaEventBusBuilder};
use crate::config::KafkaConfig;
use crate::error::map_kafka_error;
use crate::inner::KafkaInner;

/// Apache Kafka-backed event bus.
///
/// Publishes events as JSON messages to Kafka topics and consumes them
/// via background `StreamConsumer` tasks that dispatch to locally registered handlers.
///
/// `KafkaEventBus` is `Clone` — all clones share the same underlying producer
/// and handler registry.
///
/// # Limitations
///
/// - `emit_and_wait` publishes to Kafka AND waits for **local** handlers only.
///   It cannot wait for handlers on remote instances.
/// - One event type per topic (the deserializer is registered on first `subscribe`).
#[derive(Clone)]
pub struct KafkaEventBus {
    pub(crate) inner: Arc<KafkaInner>,
}

impl KafkaEventBus {
    /// Create a builder for configuring and connecting a `KafkaEventBus`.
    pub fn builder(config: KafkaConfig) -> KafkaEventBusBuilder {
        KafkaEventBusBuilder::new(config)
    }

    /// Resolve the topic name for an event type.
    fn resolve_topic<E: 'static>(&self) -> String {
        self.inner.state.resolve_topic::<E>()
    }

    /// Ensure a topic exists in Kafka (idempotent, cached).
    async fn ensure_topic(&self, topic_name: &str) -> Result<(), EventBusError> {
        if !self.inner.config.auto_create {
            return Ok(());
        }

        if self.inner.state.is_topic_ensured(topic_name) {
            return Ok(());
        }

        ensure_topic_exists(&self.inner.config, topic_name).await?;
        self.inner.state.set_topic_ensured(topic_name);
        Ok(())
    }

    /// Publish a serialized event to Kafka.
    async fn publish(
        &self,
        topic_name: &str,
        payload: Vec<u8>,
        metadata: &EventMetadata,
    ) -> Result<(), EventBusError> {
        self.ensure_topic(topic_name).await?;

        let pairs = encode_metadata(metadata);

        let mut record = FutureRecord::to(topic_name).payload(&payload);

        // Use partition_key as the Kafka message key
        if let Some(ref key) = metadata.partition_key {
            record = record.key(key);
        }

        // Encode metadata as Kafka headers
        let header_storage: Vec<(String, Vec<u8>)> = pairs
            .into_iter()
            .map(|(k, v)| (k, v.into_bytes()))
            .collect();

        let mut owned_headers = rdkafka::message::OwnedHeaders::new();
        for (k, v) in &header_storage {
            owned_headers = owned_headers.insert(rdkafka::message::Header {
                key: k,
                value: Some(v),
            });
        }
        record = record.headers(owned_headers);

        self.inner
            .producer
            .send(record, Duration::from_secs(5))
            .await
            .map_err(|(e, _)| map_kafka_error(e))?;

        Ok(())
    }
}

impl EventBus for KafkaEventBus {
    fn register_topic<E: 'static>(&self, topic: &str) -> impl Future<Output = ()> + Send {
        let inner = self.inner.clone();
        let topic = topic.to_string();
        async move {
            let type_id = TypeId::of::<E>();
            inner.state.topic_registry.write().unwrap_or_else(|e| e.into_inner()).register_by_type_id(type_id, topic);
        }
    }

    fn configure_handler<E: 'static>(
        &self,
        handler_id: r2e_events::SubscriptionId,
        filter: Option<r2e_events::EventFilter>,
        retry_policy: Option<r2e_events::RetryPolicy>,
    ) -> impl Future<Output = ()> + Send {
        let inner = self.inner.clone();
        async move {
            inner.state.configure_handler(handler_id.0, filter, retry_policy, Some(TypeId::of::<E>())).await;
        }
    }

    fn subscribe<E, F, Fut>(
        &self,
        handler: F,
    ) -> impl Future<Output = Result<SubscriptionHandle, EventBusError>> + Send
    where
        E: DeserializeOwned + Send + Sync + 'static,
        F: Fn(EventEnvelope<E>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = HandlerResult> + Send + 'static,
    {
        let inner = self.inner.clone();
        let bus = self.clone();
        async move {
            inner.state.check_shutdown()?;

            let type_id = TypeId::of::<E>();
            let topic_name = bus.resolve_topic::<E>();

            let h: Handler = Arc::new(move |any, metadata| {
                let event = any.downcast::<E>().expect("event type mismatch");
                let envelope = EventEnvelope { event, metadata };
                Box::pin(handler(envelope))
            });

            let (id, is_first) = inner.state.register_handler::<E>(h).await;

            // If this is the first subscriber for this type, set up the consumer
            if is_first {
                bus.ensure_topic(&topic_name).await?;

                let cancel = inner.state.register_poller_cancel(type_id);

                let inner_clone = bus.inner.clone();
                let topic_clone = topic_name.clone();

                tokio::spawn(async move {
                    run_consumer(inner_clone, type_id, topic_clone, cancel).await;
                });
            }

            Ok(inner.state.build_unsubscribe_handle(type_id, id))
        }
    }

    fn subscribe_with_deserializer<E, F, Fut>(
        &self,
        deserializer: r2e_events::backend::DeserializerFn,
        handler: F,
    ) -> impl Future<Output = Result<SubscriptionHandle, EventBusError>> + Send
    where
        E: Send + Sync + 'static,
        F: Fn(EventEnvelope<E>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = HandlerResult> + Send + 'static,
    {
        let inner = self.inner.clone();
        let bus = self.clone();
        async move {
            inner.state.check_shutdown()?;

            let type_id = TypeId::of::<E>();
            let topic_name = bus.resolve_topic::<E>();

            let h: Handler = Arc::new(move |any, metadata| {
                let event = any.downcast::<E>().expect("event type mismatch");
                let envelope = EventEnvelope { event, metadata };
                Box::pin(handler(envelope))
            });

            let (id, is_first) = inner.state.register_handler_with_deserializer::<E>(h, deserializer).await;

            if is_first {
                bus.ensure_topic(&topic_name).await?;

                let cancel = inner.state.register_poller_cancel(type_id);

                let inner_clone = bus.inner.clone();
                let topic_clone = topic_name.clone();

                tokio::spawn(async move {
                    run_consumer(inner_clone, type_id, topic_clone, cancel).await;
                });
            }

            Ok(inner.state.build_unsubscribe_handle(type_id, id))
        }
    }

    fn emit<E>(&self, event: E) -> impl Future<Output = Result<(), EventBusError>> + Send
    where
        E: Serialize + Send + Sync + 'static,
    {
        let bus = self.clone();
        async move {
            bus.inner.state.check_shutdown()?;

            let payload = serde_json::to_vec(&event)
                .map_err(|e| EventBusError::Serialization(e.to_string()))?;
            let topic_name = bus.resolve_topic::<E>();
            let metadata = EventMetadata::new();
            bus.publish(&topic_name, payload, &metadata).await
        }
    }

    fn emit_with<E>(
        &self,
        event: E,
        metadata: EventMetadata,
    ) -> impl Future<Output = Result<(), EventBusError>> + Send
    where
        E: Serialize + Send + Sync + 'static,
    {
        let bus = self.clone();
        async move {
            bus.inner.state.check_shutdown()?;

            let payload = serde_json::to_vec(&event)
                .map_err(|e| EventBusError::Serialization(e.to_string()))?;
            let topic_name = bus.resolve_topic::<E>();
            bus.publish(&topic_name, payload, &metadata).await
        }
    }

    fn emit_and_wait<E>(&self, event: E) -> impl Future<Output = Result<(), EventBusError>> + Send
    where
        E: Serialize + Send + Sync + 'static,
    {
        let bus = self.clone();
        async move {
            bus.inner.state.check_shutdown()?;

            let type_id = TypeId::of::<E>();
            let payload = serde_json::to_vec(&event)
                .map_err(|e| EventBusError::Serialization(e.to_string()))?;
            let topic_name = bus.resolve_topic::<E>();
            let metadata = EventMetadata::new();

            bus.publish(&topic_name, payload.clone(), &metadata).await?;

            bus.inner
                .state
                .dispatch_local(type_id, &payload, metadata)
                .await
        }
    }

    fn emit_and_wait_with<E>(
        &self,
        event: E,
        metadata: EventMetadata,
    ) -> impl Future<Output = Result<(), EventBusError>> + Send
    where
        E: Serialize + Send + Sync + 'static,
    {
        let bus = self.clone();
        async move {
            bus.inner.state.check_shutdown()?;

            let type_id = TypeId::of::<E>();
            let payload = serde_json::to_vec(&event)
                .map_err(|e| EventBusError::Serialization(e.to_string()))?;
            let topic_name = bus.resolve_topic::<E>();

            bus.publish(&topic_name, payload.clone(), &metadata).await?;

            bus.inner
                .state
                .dispatch_local(type_id, &payload, metadata)
                .await
        }
    }

    fn clear(&self) -> impl Future<Output = ()> + Send {
        let inner = self.inner.clone();
        async move {
            inner.state.cancel_all_pollers();
            inner.state.handlers.write().await.clear();
        }
    }

    fn shutdown(
        &self,
        timeout: Duration,
    ) -> impl Future<Output = Result<(), EventBusError>> + Send {
        let inner = self.inner.clone();
        async move {
            inner.state.shutdown.store(true, Ordering::Release);

            inner.state.cancel_all_pollers();

            inner.state.wait_in_flight(timeout).await?;

            inner.state.handlers.write().await.clear();

            // Flush the producer
            inner.producer.flush(timeout).map_err(map_kafka_error)?;

            Ok(())
        }
    }
}

/// Background consumer loop for a single Kafka topic with automatic reconnection.
async fn run_consumer(
    inner: Arc<KafkaInner>,
    type_id: TypeId,
    topic_name: String,
    cancel: CancellationToken,
) {
    let max_backoff = inner.config.reconnect_max_backoff;
    let reconnect = inner.config.reconnect;
    let mut backoff = Duration::from_secs(1);

    loop {
        let start = std::time::Instant::now();
        run_consumer_inner(&inner, type_id, &topic_name, &cancel).await;

        if cancel.is_cancelled() || !reconnect {
            break;
        }

        // Reset backoff if the consumer ran successfully for a while
        if start.elapsed() > backoff * 4 {
            backoff = Duration::from_secs(1);
        }

        tracing::warn!(topic = %topic_name, "Kafka consumer disconnected, reconnecting in {backoff:?}");
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(backoff) => {},
        }
        backoff = (backoff * 2).min(max_backoff);
    }
}

async fn run_consumer_inner(
    inner: &Arc<KafkaInner>,
    type_id: TypeId,
    topic_name: &str,
    cancel: &CancellationToken,
) {
    let consumer: StreamConsumer = match inner.config.to_consumer_client_config().create() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(topic = %topic_name, "failed to create Kafka consumer: {e}");
            return;
        }
    };

    if let Err(e) = consumer.subscribe(&[topic_name]) {
        tracing::error!(topic = %topic_name, "failed to subscribe to Kafka topic: {e}");
        return;
    }

    tracing::info!(topic = %topic_name, "Kafka consumer started");

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::info!(topic = %topic_name, "Kafka consumer cancelled");
                break;
            }
            msg = consumer.recv() => {
                match msg {
                    Ok(borrowed_msg) => {
                        let payload = match borrowed_msg.payload() {
                            Some(p) => p,
                            None => {
                                tracing::warn!(topic = %topic_name, "received Kafka message with no payload");
                                continue;
                            }
                        };

                        let metadata = extract_metadata_from_kafka(&borrowed_msg);
                        inner.state.dispatch_from_poller(type_id, payload, metadata).await;
                    }
                    Err(e) => {
                        tracing::warn!(topic = %topic_name, "Kafka consumer error: {e}");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        }
    }
}

/// Extract `EventMetadata` from Kafka message headers.
fn extract_metadata_from_kafka(msg: &rdkafka::message::BorrowedMessage<'_>) -> EventMetadata {
    let mut pairs: Vec<(String, String)> = Vec::new();

    if let Some(headers) = msg.headers() {
        for header in headers.iter() {
            if let Some(value) = header.value {
                if let Ok(v) = std::str::from_utf8(value) {
                    pairs.push((header.key.to_string(), v.to_string()));
                }
            }
        }
    }

    decode_metadata(pairs.into_iter())
}
