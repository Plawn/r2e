// NOTE: The `iggy` client library is tokio-bound; any tokio APIs that originate
// from the iggy SDK (e.g. the consumer stream driver) remain on direct tokio
// and are a documented exception to the r2e_core::rt facade.
use std::any::TypeId;
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use iggy::prelude::*;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use r2e_events::backend::{
    decode_metadata, encode_metadata, spawn_completion_forwarder, DispatchOutcome, Handler,
    WatermarkTracker, COMPLETION_CHANNEL_CAPACITY, COMPLETION_DRAIN_TIMEOUT, HEADER_TIMESTAMP,
};
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
    ) -> Result<BTreeMap<HeaderKey, HeaderValue>, EventBusError> {
        let pairs = encode_metadata(metadata);
        let mut headers = BTreeMap::new();

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

                r2e_core::rt::spawn(async move {
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

                r2e_core::rt::spawn(async move {
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

            // Dispatch locally and wait FIRST: dispatch_local records the local
            // outcome so the poller dedups the broker copy. Publishing before
            // this races the poller consuming that copy. If the local dispatch
            // errors, don't publish.
            bus.inner
                .state
                .dispatch_local(type_id, &payload, metadata.clone())
                .await?;

            // Then publish to Iggy.
            bus.publish(&topic_name, payload, &metadata).await
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

            // Dispatch locally and wait FIRST: dispatch_local records the local
            // outcome so the poller dedups the broker copy. Publishing before
            // this races the poller consuming that copy. If the local dispatch
            // errors, don't publish.
            bus.inner
                .state
                .dispatch_local(type_id, &payload, metadata.clone())
                .await?;

            // Then publish to Iggy.
            bus.publish(&topic_name, payload, &metadata).await
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
            _ = r2e_core::rt::sleep(backoff) => {},
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
            // At-least-once delivery: disable broker-side auto-commit and store
            // offsets manually only after local handlers complete (see below).
            .auto_commit(AutoCommit::Disabled)
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

    // At-least-once, pipelined: handlers are dispatched as messages arrive
    // (permit-bounded inside `dispatch_from_poller_tracked`) and their
    // completions are forwarded — out of order — to a bounded channel. A
    // dedicated `select!` arm applies each completion to a persistent
    // `WatermarkTracker` and stores the resulting commit offset. The poll loop
    // never awaits a handler outcome inline, so a single hung handler cannot
    // park the loop: `cancel.cancelled()` stays live and shutdown never hangs.
    //
    // The watermark tracker lives for the whole poller session (across
    // batches). A nacked offset pins its partition's commit boundary for the
    // rest of the session, so nothing at or above a nacked offset is ever
    // stored — fixing the cross-batch loss where a later batch could commit
    // past an earlier nack.
    //
    // `ready_chunks` still groups the messages the consumer has buffered from a
    // server poll into one batch (amortizing the drain), but outcomes are no
    // longer awaited before pulling the next batch.
    let batch_capacity = (inner.config.poll_batch_size.max(1)) as usize;
    let mut batches = futures_util::StreamExt::ready_chunks(consumer, batch_capacity);

    let mut tracker: WatermarkTracker<u32, u64> = WatermarkTracker::new();
    let (tx, mut rx) =
        tokio::sync::mpsc::channel::<((u32, u64), DispatchOutcome)>(COMPLETION_CHANNEL_CAPACITY);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::info!(topic = %topic_name, "poller cancelled");
                break;
            }
            // Apply resolved completions: advance the watermark and store the
            // commit offset when it moves. `recv` yields `None` only once every
            // sender (the loop's `tx` and all forwarders) is dropped, which
            // cannot happen while the loop holds `tx`, so this arm never
            // spuriously ends the loop.
            Some(((partition_id, offset), outcome)) = rx.recv() => {
                apply_completion(
                    &mut tracker,
                    batches.get_ref(),
                    topic_name,
                    partition_id,
                    offset,
                    outcome,
                )
                .await;
            }
            batch = futures_util::StreamExt::next(&mut batches) => {
                let batch = match batch {
                    Some(batch) => batch,
                    None => {
                        tracing::info!(topic = %topic_name, "consumer stream ended");
                        break;
                    }
                };

                // Dispatch every message in the batch and forward its
                // completion; do NOT await outcomes here.
                for item in &batch {
                    match item {
                        Ok(received) => {
                            let partition_id = received.partition_id;
                            let offset = received.message.header.offset;
                            // Record receipt before dispatching: the watermark
                            // is gated by the set of received offsets.
                            tracker.on_receive(partition_id, offset);
                            let metadata = extract_metadata_from_message(&received.message);
                            let completion = inner.state
                                .dispatch_from_poller_tracked(
                                    type_id,
                                    &received.message.payload,
                                    metadata,
                                )
                                .await;
                            spawn_completion_forwarder(
                                completion,
                                (partition_id, offset),
                                tx.clone(),
                            );
                        }
                        Err(e) => {
                            tracing::warn!(topic = %topic_name, "poll error: {e}");
                        }
                    }
                }
            }
        }
    }

    // The loop has exited (cancelled or stream ended). Drop the loop's sender so
    // the channel closes once the in-flight forwarders finish, then drain
    // remaining completions best-effort within the deadline. Acks still advance
    // the watermark and store their offset; anything undrained is simply
    // redelivered on restart.
    drop(tx);
    let consumer = batches.get_ref();
    let drain = async {
        while let Some(((partition_id, offset), outcome)) = rx.recv().await {
            apply_completion(&mut tracker, consumer, topic_name, partition_id, offset, outcome)
                .await;
        }
    };
    if r2e_core::rt::timeout(COMPLETION_DRAIN_TIMEOUT, drain).await.is_err() {
        tracing::warn!(
            topic = %topic_name,
            "timed out draining completions on poller shutdown; \
             undrained messages will be redelivered on restart"
        );
    }
}

/// Apply one resolved dispatch completion to the commit watermark.
///
/// On Ack, advance the tracker and store the commit offset if the watermark
/// moved. On Nack, pin the partition (nothing at or above this offset commits
/// again for the tracker's lifetime) and warn — the message is redelivered on
/// restart.
async fn apply_completion(
    tracker: &mut WatermarkTracker<u32, u64>,
    consumer: &IggyConsumer,
    topic_name: &str,
    partition_id: u32,
    offset: u64,
    outcome: DispatchOutcome,
) {
    match outcome {
        DispatchOutcome::Ack => {
            if let Some(commit) = tracker.on_ack(partition_id, offset) {
                if let Err(e) = consumer.store_offset(commit, Some(partition_id)).await {
                    tracing::warn!(
                        topic = %topic_name,
                        partition_id,
                        offset = commit,
                        "failed to store Iggy consumer offset: {e}"
                    );
                }
            }
        }
        DispatchOutcome::Nack => {
            tracker.on_nack(partition_id, offset);
            tracing::warn!(
                topic = %topic_name,
                partition_id,
                offset,
                "handler nacked without DLQ capture; partition pinned at this \
                 offset, message redelivered on restart"
            );
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
