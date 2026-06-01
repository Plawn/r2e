use std::any::TypeId;
use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use pulsar::consumer::Consumer;
use pulsar::producer::Message as ProducerMessage;
use pulsar::{TokioExecutor, Error as PulsarError};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use r2e_events::backend::{decode_metadata, encode_metadata, Handler, HEADER_PARTITION_KEY};
use r2e_events::{
    EventBus, EventBusError, EventEnvelope, EventMetadata, HandlerResult, SubscriptionHandle,
};

use crate::builder::PulsarEventBusBuilder;
use crate::config::PulsarConfig;
use crate::error::map_pulsar_error;
use crate::inner::PulsarInner;

/// Apache Pulsar-backed event bus.
///
/// Publishes events as JSON messages to Pulsar topics and consumes them
/// via background consumer tasks that dispatch to locally registered handlers.
///
/// `PulsarEventBus` is `Clone` — all clones share the same underlying connection
/// and handler registry.
///
/// # Limitations
///
/// - `emit_and_wait` publishes to Pulsar AND waits for **local** handlers only.
///   It cannot wait for handlers on remote instances.
/// - One event type per topic (the deserializer is registered on first `subscribe`).
#[derive(Clone)]
pub struct PulsarEventBus {
    pub(crate) inner: Arc<PulsarInner>,
}

impl PulsarEventBus {
    /// Create a builder for configuring and connecting a `PulsarEventBus`.
    pub fn builder(config: PulsarConfig) -> PulsarEventBusBuilder {
        PulsarEventBusBuilder::new(config)
    }

    /// Resolve the topic name for an event type.
    fn resolve_topic<E: 'static>(&self) -> String {
        self.inner.state.resolve_topic::<E>()
    }

    /// Build the full Pulsar topic name (with prefix).
    fn full_topic(&self, topic_name: &str) -> String {
        self.inner.config.full_topic_name(topic_name)
    }

    /// Get or create a cached producer for a topic.
    async fn get_or_create_producer(
        &self,
        full_topic: &str,
    ) -> Result<(), EventBusError> {
        let mut producers = self.inner.producers.lock().await;
        if producers.contains_key(full_topic) {
            return Ok(());
        }

        let producer = self
            .inner
            .pulsar
            .producer()
            .with_topic(full_topic)
            .build()
            .await
            .map_err(map_pulsar_error)?;

        producers.insert(full_topic.to_string(), producer);
        Ok(())
    }

    /// Build message properties from `EventMetadata`.
    fn build_properties(metadata: &EventMetadata) -> HashMap<String, String> {
        let pairs = encode_metadata(metadata);
        pairs.into_iter().collect()
    }

    /// Publish a serialized event to Pulsar.
    async fn publish(
        &self,
        topic_name: &str,
        payload: Vec<u8>,
        metadata: &EventMetadata,
    ) -> Result<(), EventBusError> {
        let full_topic = self.full_topic(topic_name);

        // Ensure producer exists
        self.get_or_create_producer(&full_topic).await?;

        let properties = Self::build_properties(metadata);

        let partition_key = metadata.partition_key.clone();

        let msg = ProducerMessage {
            payload,
            properties,
            partition_key,
            ordering_key: None,
            replicate_to: Vec::new(),
            event_time: None,
            schema_version: None,
            deliver_at_time: None,
        };

        // Lock the producers only for the send; release before awaiting the broker receipt.
        let receipt = {
            let mut producers = self.inner.producers.lock().await;
            let producer = producers
                .get_mut(&full_topic)
                .ok_or_else(|| EventBusError::Other(format!("producer not found for {full_topic}")))?;

            producer
                .send_non_blocking(msg)
                .await
                .map_err(|e: PulsarError| map_pulsar_error(e))?
        };

        // Await the broker acknowledgement without holding the Mutex.
        receipt
            .await
            .map_err(|e| EventBusError::Other(format!("send receipt error: {e}")))?;

        Ok(())
    }
}

impl EventBus for PulsarEventBus {
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

            // If this is the first subscriber for this type, set up the consumer poller
            if is_first {
                let full_topic = bus.full_topic(&topic_name);

                let cancel = inner.state.register_poller_cancel(type_id);

                let inner_clone = bus.inner.clone();
                let config = bus.inner.config.clone();

                tokio::spawn(async move {
                    run_poller(inner_clone, type_id, full_topic, config, cancel).await;
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
                let full_topic = bus.full_topic(&topic_name);

                let cancel = inner.state.register_poller_cancel(type_id);

                let inner_clone = bus.inner.clone();
                let config = bus.inner.config.clone();

                tokio::spawn(async move {
                    run_poller(inner_clone, type_id, full_topic, config, cancel).await;
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

            // Publish to Pulsar
            bus.publish(&topic_name, payload.clone(), &metadata).await?;

            // Also dispatch locally and wait
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

            // Publish to Pulsar
            bus.publish(&topic_name, payload.clone(), &metadata).await?;

            // Also dispatch locally and wait
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
        timeout: std::time::Duration,
    ) -> impl Future<Output = Result<(), EventBusError>> + Send {
        let inner = self.inner.clone();
        async move {
            // Set shutdown flag
            inner.state.shutdown.store(true, Ordering::Release);

            // Cancel all pollers
            inner.state.cancel_all_pollers();

            // Wait for in-flight handlers to complete
            inner.state.wait_in_flight(timeout).await?;

            // Clear handlers
            inner.state.handlers.write().await.clear();

            // Close all cached producers
            let mut producers = inner.producers.lock().await;
            producers.clear();

            Ok(())
        }
    }
}

/// Background consumer loop for a single topic with automatic reconnection.
async fn run_poller(
    inner: Arc<PulsarInner>,
    type_id: TypeId,
    full_topic: String,
    config: PulsarConfig,
    cancel: CancellationToken,
) {
    let max_backoff = config.reconnect_max_backoff;
    let reconnect = config.reconnect;
    let mut backoff = std::time::Duration::from_secs(1);

    loop {
        let start = std::time::Instant::now();
        run_poller_inner(&inner, type_id, &full_topic, &config, &cancel).await;

        if cancel.is_cancelled() || !reconnect {
            break;
        }

        // Reset backoff if the poller ran successfully for a while
        if start.elapsed() > backoff * 4 {
            backoff = std::time::Duration::from_secs(1);
        }

        tracing::warn!(topic = %full_topic, "Pulsar poller disconnected, reconnecting in {backoff:?}");
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(backoff) => {},
        }
        backoff = (backoff * 2).min(max_backoff);
    }
}

async fn run_poller_inner(
    inner: &Arc<PulsarInner>,
    type_id: TypeId,
    full_topic: &str,
    config: &PulsarConfig,
    cancel: &CancellationToken,
) {
    let consumer_result: Result<Consumer<Vec<u8>, TokioExecutor>, PulsarError> = inner
        .pulsar
        .consumer()
        .with_topic(full_topic)
        .with_subscription(&config.subscription)
        .with_subscription_type(config.subscription_type.to_sub_type())
        .with_consumer_name(format!("r2e-consumer-{}", full_topic))
        .build()
        .await;

    let mut consumer = match consumer_result {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(topic = %full_topic, "failed to create Pulsar consumer: {e}");
            return;
        }
    };

    tracing::info!(topic = %full_topic, "poller started");

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::info!(topic = %full_topic, "poller cancelled");
                break;
            }
            msg = futures_util::StreamExt::next(&mut consumer) => {
                match msg {
                    Some(Ok(received)) => {
                        let metadata = extract_metadata_from_message(&received);
                        let payload = &received.payload.data;

                        inner.state.dispatch_from_poller(type_id, payload, metadata).await;

                        // Acknowledge the message after dispatch
                        if let Err(e) = consumer.ack(&received).await {
                            tracing::warn!(topic = %full_topic, "failed to ack message: {e}");
                        }
                    }
                    Some(Err(e)) => {
                        tracing::warn!(topic = %full_topic, "consumer error: {e}");
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                    None => {
                        tracing::info!(topic = %full_topic, "consumer stream ended");
                        break;
                    }
                }
            }
        }
    }
}

/// Extract `EventMetadata` from Pulsar message properties.
fn extract_metadata_from_message(
    message: &pulsar::consumer::Message<Vec<u8>>,
) -> EventMetadata {
    let mut pairs: Vec<(String, String)> = Vec::new();

    // Extract properties from the message metadata
    for kv in &message.payload.metadata.properties {
        pairs.push((kv.key.clone(), kv.value.clone()));
    }

    // Add partition key if present
    if let Some(ref key) = message.payload.metadata.partition_key {
        if !key.is_empty() {
            pairs.push((HEADER_PARTITION_KEY.to_string(), key.clone()));
        }
    }

    decode_metadata(pairs.into_iter())
}
