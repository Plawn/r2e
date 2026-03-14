use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use iggy::prelude::*;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use r2e_events::{
    EventBus, EventBusError, EventEnvelope, EventMetadata, HandlerResult, SubscriptionHandle,
    SubscriptionId,
};

use crate::builder::IggyEventBusBuilder;
use crate::config::IggyConfig;
use crate::dispatch::{DeserializerFn, Handler, HandlerEntry, TopicHandlers};
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
    async fn resolve_topic<E: 'static>(&self) -> String {
        let type_id = TypeId::of::<E>();
        let type_name = std::any::type_name::<E>();
        let reg = self.inner.topic_registry.read().await;
        reg.resolve(type_id, type_name)
    }

    /// Ensure a topic exists in Iggy (idempotent, cached).
    async fn ensure_topic(&self, topic_name: &str) -> Result<(), EventBusError> {
        if !self.inner.config.auto_create {
            return Ok(());
        }

        {
            let ensured = self.inner.ensured_topics.lock().await;
            if ensured.contains(topic_name) {
                return Ok(());
            }
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

        self.inner
            .ensured_topics
            .lock()
            .await
            .insert(topic_name.to_string());
        Ok(())
    }

    /// Build Iggy message headers from `EventMetadata`.
    fn build_headers(
        metadata: &EventMetadata,
    ) -> Result<HashMap<HeaderKey, HeaderValue>, EventBusError> {
        let mut headers = HashMap::new();

        headers.insert(
            HeaderKey::try_from("r2e-event-id")
                .map_err(|e: IggyError| EventBusError::Serialization(e.to_string()))?,
            HeaderValue::try_from(metadata.event_id.to_string().as_str())
                .map_err(|e: IggyError| EventBusError::Serialization(e.to_string()))?,
        );

        headers.insert(
            HeaderKey::try_from("r2e-timestamp")
                .map_err(|e: IggyError| EventBusError::Serialization(e.to_string()))?,
            HeaderValue::try_from(metadata.timestamp.to_string().as_str())
                .map_err(|e: IggyError| EventBusError::Serialization(e.to_string()))?,
        );

        if let Some(ref cid) = metadata.correlation_id {
            headers.insert(
                HeaderKey::try_from("r2e-correlation-id")
                    .map_err(|e: IggyError| EventBusError::Serialization(e.to_string()))?,
                HeaderValue::try_from(cid.as_str())
                    .map_err(|e: IggyError| EventBusError::Serialization(e.to_string()))?,
            );
        }

        for (k, v) in &metadata.headers {
            let header_key = format!("r2e-h-{k}");
            headers.insert(
                HeaderKey::try_from(header_key.as_str())
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

    /// Dispatch to local handlers (for emit_and_wait).
    async fn dispatch_local(
        &self,
        type_id: TypeId,
        payload: &[u8],
        metadata: EventMetadata,
    ) -> Result<(), EventBusError> {
        let map = self.inner.handlers.read().await;
        let topic_handlers = match map.get(&type_id) {
            Some(th) => th,
            None => return Ok(()),
        };

        let event =
            (topic_handlers.deserializer)(payload).map_err(EventBusError::Serialization)?;

        let mut tasks = Vec::new();
        for entry in &topic_handlers.entries {
            let h = entry.handler.clone();
            let e = event.clone();
            let m = metadata.clone();
            tasks.push(tokio::spawn(async move { h(e, m).await }));
        }

        for task in tasks {
            if let Ok(HandlerResult::Nack(reason)) = task.await {
                tracing::warn!("event handler returned Nack: {reason}");
            }
        }

        Ok(())
    }
}

impl EventBus for IggyEventBus {
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
            if inner.shutdown.load(Ordering::SeqCst) {
                return Err(EventBusError::Shutdown);
            }

            let type_id = TypeId::of::<E>();
            let id = inner.next_id.fetch_add(1, Ordering::Relaxed);
            let topic_name = bus.resolve_topic::<E>().await;

            let h: Handler = Arc::new(move |any, metadata| {
                let event = any.downcast::<E>().expect("event type mismatch");
                let envelope = EventEnvelope { event, metadata };
                Box::pin(handler(envelope))
            });

            let mut map = inner.handlers.write().await;
            let is_first = !map.contains_key(&type_id);

            let topic_entry = map.entry(type_id).or_insert_with(|| {
                let deser: DeserializerFn = Arc::new(|bytes: &[u8]| {
                    serde_json::from_slice::<E>(bytes)
                        .map(|e| Arc::new(e) as Arc<dyn Any + Send + Sync>)
                        .map_err(|e| e.to_string())
                });
                TopicHandlers {
                    entries: Vec::new(),
                    deserializer: deser,
                }
            });

            topic_entry.entries.push(HandlerEntry { id, handler: h });
            drop(map);

            // If this is the first subscriber for this type, set up the poller
            if is_first {
                bus.ensure_topic(&topic_name).await?;

                let cancel = CancellationToken::new();
                inner
                    .poller_cancels
                    .lock()
                    .await
                    .insert(type_id, cancel.clone());

                let inner_clone = bus.inner.clone();
                let topic_clone = topic_name.clone();

                tokio::spawn(async move {
                    run_poller(inner_clone, type_id, topic_clone, cancel).await;
                });
            }

            // Build unsubscribe closure
            let inner_for_unsub = bus.inner.clone();
            Ok(SubscriptionHandle::new(
                SubscriptionId(id),
                move || {
                    let inner = inner_for_unsub.clone();
                    tokio::spawn(async move {
                        let mut map = inner.handlers.write().await;
                        if let Some(th) = map.get_mut(&type_id) {
                            th.entries.retain(|e| e.id != id);
                            if th.entries.is_empty() {
                                map.remove(&type_id);
                                // Cancel the poller
                                let mut cancels = inner.poller_cancels.lock().await;
                                if let Some(cancel) = cancels.remove(&type_id) {
                                    cancel.cancel();
                                }
                            }
                        }
                    });
                },
            ))
        }
    }

    fn emit<E>(&self, event: E) -> impl Future<Output = Result<(), EventBusError>> + Send
    where
        E: Serialize + Send + Sync + 'static,
    {
        let bus = self.clone();
        async move {
            if bus.inner.shutdown.load(Ordering::SeqCst) {
                return Err(EventBusError::Shutdown);
            }

            let payload = serde_json::to_vec(&event)
                .map_err(|e| EventBusError::Serialization(e.to_string()))?;
            let topic_name = bus.resolve_topic::<E>().await;
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
            if bus.inner.shutdown.load(Ordering::SeqCst) {
                return Err(EventBusError::Shutdown);
            }

            let payload = serde_json::to_vec(&event)
                .map_err(|e| EventBusError::Serialization(e.to_string()))?;
            let topic_name = bus.resolve_topic::<E>().await;
            bus.publish(&topic_name, payload, &metadata).await
        }
    }

    fn emit_and_wait<E>(&self, event: E) -> impl Future<Output = Result<(), EventBusError>> + Send
    where
        E: Serialize + Send + Sync + 'static,
    {
        let bus = self.clone();
        async move {
            if bus.inner.shutdown.load(Ordering::SeqCst) {
                return Err(EventBusError::Shutdown);
            }

            let type_id = TypeId::of::<E>();
            let payload = serde_json::to_vec(&event)
                .map_err(|e| EventBusError::Serialization(e.to_string()))?;
            let topic_name = bus.resolve_topic::<E>().await;
            let metadata = EventMetadata::new();

            // Publish to Iggy
            bus.publish(&topic_name, payload.clone(), &metadata).await?;

            // Also dispatch locally and wait
            bus.dispatch_local(type_id, &payload, metadata).await
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
            if bus.inner.shutdown.load(Ordering::SeqCst) {
                return Err(EventBusError::Shutdown);
            }

            let type_id = TypeId::of::<E>();
            let payload = serde_json::to_vec(&event)
                .map_err(|e| EventBusError::Serialization(e.to_string()))?;
            let topic_name = bus.resolve_topic::<E>().await;

            // Publish to Iggy
            bus.publish(&topic_name, payload.clone(), &metadata).await?;

            // Also dispatch locally and wait
            bus.dispatch_local(type_id, &payload, metadata).await
        }
    }

    fn clear(&self) -> impl Future<Output = ()> + Send {
        let inner = self.inner.clone();
        async move {
            // Cancel all pollers
            let mut cancels: HashMap<TypeId, CancellationToken> =
                std::mem::take(&mut *inner.poller_cancels.lock().await);
            for (_, cancel) in cancels.drain() {
                cancel.cancel();
            }

            // Clear handlers
            inner.handlers.write().await.clear();
        }
    }

    fn shutdown(
        &self,
        timeout: std::time::Duration,
    ) -> impl Future<Output = Result<(), EventBusError>> + Send {
        let inner = self.inner.clone();
        async move {
            // Set shutdown flag
            inner.shutdown.store(true, Ordering::SeqCst);

            // Cancel all pollers
            let mut cancels: HashMap<TypeId, CancellationToken> =
                std::mem::take(&mut *inner.poller_cancels.lock().await);
            for (_, cancel) in cancels.drain() {
                cancel.cancel();
            }

            // Wait for in-flight handlers to complete
            if inner.in_flight.load(Ordering::SeqCst) > 0 {
                let wait = async {
                    loop {
                        if inner.in_flight.load(Ordering::SeqCst) == 0 {
                            return;
                        }
                        inner.in_flight_zero.notified().await;
                    }
                };
                if tokio::time::timeout(timeout, wait).await.is_err() {
                    inner.handlers.write().await.clear();
                    return Err(EventBusError::Other(format!(
                        "shutdown timed out with {} handlers still in flight",
                        inner.in_flight.load(Ordering::SeqCst)
                    )));
                }
            }

            // Clear handlers
            inner.handlers.write().await.clear();

            // Disconnect client
            if let Err(e) = inner.client.shutdown().await {
                tracing::warn!("error disconnecting Iggy client: {e}");
            }

            Ok(())
        }
    }
}

/// Background poller loop for a single topic.
async fn run_poller(
    inner: Arc<IggyInner>,
    type_id: TypeId,
    topic_name: String,
    cancel: CancellationToken,
) {
    let consumer_result = inner.client.consumer_group(
        &inner.config.consumer_group,
        &inner.config.stream_name,
        &topic_name,
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
                        dispatch_from_poller(&received.message, type_id, &inner).await;
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

/// Deserialize and dispatch a polled message to local handlers.
async fn dispatch_from_poller(message: &IggyMessage, type_id: TypeId, inner: &Arc<IggyInner>) {
    let map = inner.handlers.read().await;
    let topic_handlers = match map.get(&type_id) {
        Some(th) => th,
        None => return,
    };

    let event = match (topic_handlers.deserializer)(&message.payload) {
        Ok(e) => e,
        Err(err) => {
            tracing::error!("failed to deserialize event: {err}");
            return;
        }
    };

    let metadata = extract_metadata_from_message(message);

    for entry in &topic_handlers.entries {
        let h = entry.handler.clone();
        let e = event.clone();
        let m = metadata.clone();

        inner.in_flight.fetch_add(1, Ordering::SeqCst);

        let inner_clone = inner.clone();
        tokio::spawn(async move {
            let result = h(e, m).await;
            if inner_clone
                .in_flight
                .fetch_sub(1, Ordering::SeqCst)
                == 1
            {
                inner_clone.in_flight_zero.notify_waiters();
            }
            if let HandlerResult::Nack(reason) = result {
                tracing::warn!("event handler returned Nack: {reason}");
            }
        });
    }
}

/// Extract `EventMetadata` from Iggy message headers.
fn extract_metadata_from_message(message: &IggyMessage) -> EventMetadata {
    let mut metadata = EventMetadata::new();
    metadata.timestamp = message.header.timestamp;

    if let Ok(Some(headers)) = message.user_headers_map() {
        for (key, value) in &headers {
            let key_str = match key.as_str() {
                Ok(s) => s,
                Err(_) => continue,
            };
            match key_str {
                "r2e-event-id" => {
                    if let Ok(s) = value.as_str() {
                        if let Ok(id) = s.parse::<u64>() {
                            metadata.event_id = id;
                        }
                    }
                }
                "r2e-correlation-id" => {
                    if let Ok(s) = value.as_str() {
                        metadata.correlation_id = Some(s.to_string());
                    }
                }
                k if k.starts_with("r2e-h-") => {
                    if let Ok(v) = value.as_str() {
                        metadata
                            .headers
                            .insert(k.trim_start_matches("r2e-h-").to_string(), v.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    metadata
}
