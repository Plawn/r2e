use std::any::TypeId;
use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use iggy::prelude::*;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use r2e_events::backend::{decode_metadata, encode_metadata, Handler, HEADER_TIMESTAMP};
use r2e_events::{
    EventBus, EventBusError, EventEnvelope, EventMetadata, HandlerResult, SubscriptionHandle,
};

use crate::builder::IggyEventBusBuilder;
use crate::config::IggyConfig;
use crate::error::map_iggy_error;
use crate::inner::IggyInner;

/// Apache Iggy-backed event bus.
///
/// Publishes events as JSON messages to Iggy topics and consumes them
/// via background poller tasks that dispatch to locally registered handlers.
///
/// `IggyEventBus` is `Clone` — all clones share the same underlying connection
/// and handler registry.
///
/// # Limitations
///
/// - `emit_and_wait` publishes to Iggy AND waits for **local** handlers only.
///   It cannot wait for handlers on remote instances.
/// - One event type per topic (the deserializer is registered on first `subscribe`).
#[derive(Clone)]
pub struct IggyEventBus {
    pub(crate) inner: Arc<IggyInner>,
}

impl IggyEventBus {
    /// Create a builder for configuring and connecting an `IggyEventBus`.
    pub fn builder(config: IggyConfig) -> IggyEventBusBuilder {
        IggyEventBusBuilder::new(config)
    }

    /// Resolve the topic name for an event type.
    fn resolve_topic<E: 'static>(&self) -> String {
        self.inner.state.resolve_topic::<E>()
    }

    /// Ensure a topic exists in Iggy (idempotent, cached).
    async fn ensure_topic(&self, topic_name: &str) -> Result<(), EventBusError> {
        if !self.inner.config.auto_create {
            return Ok(());
        }

        if self.inner.state.is_topic_ensured(topic_name) {
            return Ok(());
        }

        let stream_id =
            Identifier::named(&self.inner.config.stream_name).map_err(map_iggy_error)?;

        match self
            .inner
            .client
            .create_topic(
                &stream_id,
                topic_name,
                self.inner.config.default_partitions,
                CompressionAlgorithm::default(),
                None,
                IggyExpiry::NeverExpire,
                MaxTopicSize::ServerDefault,
            )
            .await
        {
            Ok(_) => {
                tracing::info!(topic = %topic_name, "created Iggy topic");
            }
            Err(_) => {
                // Topic likely already exists — that's fine
            }
        }

        self.inner.state.set_topic_ensured(topic_name);
        Ok(())
    }

    /// Build Iggy message headers from `EventMetadata`.
    fn build_headers(
        metadata: &EventMetadata,
    ) -> Result<HashMap<HeaderKey, HeaderValue>, EventBusError> {
        let pairs = encode_metadata(metadata);
        let mut headers = HashMap::new();

        for (k, v) in pairs {
            // Skip partition_key — it's used for Iggy partitioning, not headers
            if k == r2e_events::backend::HEADER_PARTITION_KEY {
                continue;
            }
            headers.insert(
                HeaderKey::try_from(k.as_str())
                    .map_err(|e: IggyError| EventBusError::Serialization(e.to_string()))?,
                HeaderValue::try_from(v.as_str())
                    .map_err(|e: IggyError| EventBusError::Serialization(e.to_string()))?,
            );
        }

        Ok(headers)
    }

    /// Publish a serialized event to Iggy.
    async fn publish(
        &self,
        topic_name: &str,
        payload: Vec<u8>,
        metadata: &EventMetadata,
    ) -> Result<(), EventBusError> {
        self.ensure_topic(topic_name).await?;

        let stream_id =
            Identifier::named(&self.inner.config.stream_name).map_err(map_iggy_error)?;
        let topic_id = Identifier::named(topic_name).map_err(map_iggy_error)?;

        let partitioning = match &metadata.partition_key {
            Some(key) => Partitioning::messages_key_str(key)
                .map_err(|e| EventBusError::Serialization(e.to_string()))?,
            None => Partitioning::balanced(),
        };

        let headers = Self::build_headers(metadata)?;

        let msg = IggyMessage::builder()
            .payload(bytes::Bytes::from(payload))
            .user_headers(headers)
            .build()
            .map_err(|e| EventBusError::Serialization(e.to_string()))?;

        self.inner
            .client
            .send_messages(&stream_id, &topic_id, &partitioning, &mut [msg])
            .await
            .map_err(map_iggy_error)?;

        Ok(())
    }
}

impl EventBus for IggyEventBus {
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

            // If this is the first subscriber for this type, set up the poller
            if is_first {
                bus.ensure_topic(&topic_name).await?;

                let cancel = inner.state.register_poller_cancel(type_id);

                let inner_clone = bus.inner.clone();
                let topic_clone = topic_name.clone();

                tokio::spawn(async move {
                    run_poller(inner_clone, type_id, topic_clone, cancel).await;
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
                    run_poller(inner_clone, type_id, topic_clone, cancel).await;
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

            // Publish to Iggy
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

            // Publish to Iggy
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

            // Disconnect client
            if let Err(e) = inner.client.shutdown().await {
                tracing::warn!("error disconnecting Iggy client: {e}");
            }

            Ok(())
        }
    }
}

/// Background poller loop for a single topic with automatic reconnection.
async fn run_poller(
    inner: Arc<IggyInner>,
    type_id: TypeId,
    topic_name: String,
    cancel: CancellationToken,
) {
    let max_backoff = inner.config.reconnect_max_backoff;
    let reconnect = inner.config.reconnect;
    let mut backoff = std::time::Duration::from_secs(1);

    loop {
        let start = std::time::Instant::now();
        run_poller_inner(&inner, type_id, &topic_name, &cancel).await;

        if cancel.is_cancelled() || !reconnect {
            break;
        }

        // Reset backoff if the poller ran successfully for a while
        if start.elapsed() > backoff * 4 {
            backoff = std::time::Duration::from_secs(1);
        }

        tracing::warn!(topic = %topic_name, "Iggy poller disconnected, reconnecting in {backoff:?}");
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(backoff) => {},
        }
        backoff = (backoff * 2).min(max_backoff);
    }
}

async fn run_poller_inner(
    inner: &Arc<IggyInner>,
    type_id: TypeId,
    topic_name: &str,
    cancel: &CancellationToken,
) {
    let consumer_result = inner.client.consumer_group(
        &inner.config.consumer_group,
        &inner.config.stream_name,
        topic_name,
    );

    let mut consumer = match consumer_result {
        Ok(builder) => builder
            .auto_commit(AutoCommit::When(AutoCommitWhen::PollingMessages))
            .create_consumer_group_if_not_exists()
            .auto_join_consumer_group()
            .polling_strategy(PollingStrategy::next())
            .poll_interval(IggyDuration::from(inner.config.poll_interval))
            .batch_length(inner.config.poll_batch_size)
            .build(),
        Err(e) => {
            tracing::error!(topic = %topic_name, "failed to create Iggy consumer: {e}");
            return;
        }
    };

    if let Err(e) = consumer.init().await {
        tracing::error!(topic = %topic_name, "failed to init Iggy consumer: {e}");
        return;
    }

    tracing::info!(topic = %topic_name, "poller started");

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::info!(topic = %topic_name, "poller cancelled");
                break;
            }
            msg = futures_util::StreamExt::next(&mut consumer) => {
                match msg {
                    Some(Ok(received)) => {
                        let metadata = extract_metadata_from_message(&received.message);
                        inner.state.dispatch_from_poller(type_id, &received.message.payload, metadata).await;
                    }
                    Some(Err(e)) => {
                        tracing::warn!(topic = %topic_name, "poll error: {e}");
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                    None => {
                        tracing::info!(topic = %topic_name, "consumer stream ended");
                        break;
                    }
                }
            }
        }
    }
}

/// Extract `EventMetadata` from Iggy message headers.
fn extract_metadata_from_message(message: &IggyMessage) -> EventMetadata {
    let mut pairs: Vec<(String, String)> = Vec::new();

    // Add the Iggy-native timestamp
    pairs.push((HEADER_TIMESTAMP.to_string(), message.header.timestamp.to_string()));

    if let Ok(Some(headers)) = message.user_headers_map() {
        for (key, value) in &headers {
            let key_str = match key.as_str() {
                Ok(s) => s,
                Err(_) => continue,
            };
            let val_str = match value.as_str() {
                Ok(s) => s,
                Err(_) => continue,
            };
            // Skip timestamp from user headers — we use the Iggy native one
            if key_str == HEADER_TIMESTAMP {
                continue;
            }
            pairs.push((key_str.to_string(), val_str.to_string()));
        }
    }

    decode_metadata(pairs.into_iter())
}
