// NOTE: The `rdkafka` (librdkafka) consumer is tokio-bound; any tokio APIs
// that originate from the rdkafka SDK remain on direct tokio and are a
// documented exception to the r2e_core::rt facade.
use std::any::TypeId;
use std::future::Future;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use rdkafka::consumer::{
    BaseConsumer, CommitMode, Consumer, ConsumerContext, Rebalance, StreamConsumer,
};
use rdkafka::message::Headers;
use rdkafka::ClientContext;
use rdkafka::producer::{FutureRecord, Producer};
use rdkafka::Message;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use r2e_events::backend::{
    decode_metadata, encode_metadata, spawn_completion_forwarder, DispatchOutcome, Handler,
    WatermarkTracker, COMPLETION_CHANNEL_CAPACITY, COMPLETION_DRAIN_TIMEOUT,
};
use r2e_events::{
    EventBus, EventBusError, EventEnvelope, EventMetadata, HandlerResult, SubscriptionHandle,
};

use crate::builder::{ensure_topic_exists, KafkaEventBusBuilder};
use crate::config::KafkaConfig;
use crate::error::map_kafka_error;
use crate::inner::KafkaInner;

/// Interval between periodic offset commits when `enable_auto_commit` is
/// `false` — librdkafka's periodic committer is off, so the consume loop drives
/// commits of the offsets it has stored after handlers acked.
const MANUAL_COMMIT_INTERVAL: Duration = Duration::from_secs(5);

/// Per-partition commit-watermark tracker shared between the rebalance callback
/// (driver thread) and the consume loop. Guarded by a `std::sync::Mutex` with
/// short, await-free critical sections — the callback runs on librdkafka's
/// driver thread, so an async mutex would be wrong here.
type SharedTracker = Arc<std::sync::Mutex<WatermarkTracker<i32, i64>>>;

/// rdkafka consumer context that resets watermark tracking on partition
/// revoke. Without this, a revoke+reassign inside `consumer.recv()` leaves the
/// tracker's `stored` guard suppressing re-commits of redelivered offsets.
struct R2eConsumerContext {
    tracker: SharedTracker,
}

impl ClientContext for R2eConsumerContext {}

impl ConsumerContext for R2eConsumerContext {
    fn pre_rebalance(&self, _base: &BaseConsumer<Self>, rebalance: &Rebalance<'_>) {
        // On revoke, forget the revoked partitions so redelivered offsets are
        // re-tracked from scratch after reassignment. Short std-mutex hold, no
        // awaits — safe on the driver thread.
        if let Rebalance::Revoke(tpl) = rebalance {
            let mut tracker = self.tracker.lock().unwrap_or_else(|e| e.into_inner());
            for elem in tpl.elements() {
                let partition = elem.partition();
                tracker.remove_partition(&partition);
            }
        }
    }
}

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

                r2e_core::rt::spawn(async move {
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

                r2e_core::rt::spawn(async move {
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

            // Dispatch locally (recording the outcome for poller dedup) BEFORE
            // publishing — publishing first races the poller consuming the
            // broker copy before the local outcome is recorded.
            bus.inner
                .state
                .dispatch_local(type_id, &payload, metadata.clone())
                .await?;

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

            // Dispatch locally (recording the outcome for poller dedup) BEFORE
            // publishing — publishing first races the poller consuming the
            // broker copy before the local outcome is recorded.
            bus.inner
                .state
                .dispatch_local(type_id, &payload, metadata.clone())
                .await?;

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
        timeout: Duration,
    ) -> impl Future<Output = Result<(), EventBusError>> + Send {
        let inner = self.inner.clone();
        async move {
            inner.state.shutdown.store(true, Ordering::Release);

            inner.state.cancel_all_pollers();

            inner.state.wait_in_flight(timeout).await?;

            inner.state.handlers.write().await.clear();

            // Flush the producer. `flush` blocks the calling thread until the
            // queue drains, so run it on the blocking pool rather than stalling
            // the async runtime thread during shutdown. `FutureProducer` is an
            // `Arc` handle and cheap to clone.
            let producer = inner.producer.clone();
            match r2e_core::rt::spawn_blocking(move || producer.flush(timeout)).await {
                Ok(res) => res.map_err(map_kafka_error)?,
                Err(join_err) => {
                    // The blocking flush task panicked or was cancelled. On
                    // shutdown we log and continue rather than propagating a
                    // panic — the process is going away regardless.
                    tracing::warn!(error = %join_err, "kafka producer flush task failed during shutdown");
                }
            }

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
            _ = r2e_core::rt::sleep(backoff) => {},
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
    // At-least-once delivery: offsets are stored only after local handlers
    // complete. Handlers run concurrently (pipelined), so completions arrive
    // out of order; the tracker advances each partition's commit watermark over
    // the contiguous prefix of acked offsets. Fresh per consumer lifetime — on
    // reconnect it is dropped and the new consumer resumes from the last commit.
    // Shared with the rebalance callback so revoked partitions reset their state.
    let tracker: SharedTracker = Arc::new(std::sync::Mutex::new(WatermarkTracker::new()));

    let context = R2eConsumerContext { tracker: tracker.clone() };
    let consumer: StreamConsumer<R2eConsumerContext> =
        match inner.config.to_consumer_client_config().create_with_context(context) {
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

    // When the public `enable_auto_commit` is false, librdkafka never commits
    // the offsets we store, so the loop must commit them itself on a timer.
    let manual_commit = !inner.config.enable_auto_commit;
    let mut commit_interval = r2e_core::rt::interval(MANUAL_COMMIT_INTERVAL);

    // Completed dispatches report (key, outcome) back to this loop on a bounded
    // channel; the forwarder applies backpressure once the loop falls behind.
    let (completion_tx, mut completion_rx) = tokio::sync::mpsc::channel::<((i32, i64), DispatchOutcome)>(
        COMPLETION_CHANNEL_CAPACITY,
    );

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::info!(topic = %topic_name, "Kafka consumer cancelled");
                break;
            }
            // Only active with auto-commit disabled: commit the offsets stored
            // after handlers acked. `Async` keeps the loop responsive.
            _ = commit_interval.tick(), if manual_commit => {
                if let Err(e) = consumer.commit_consumer_state(CommitMode::Async) {
                    // ERR__NO_OFFSET (nothing new to commit) is benign here.
                    tracing::debug!(topic = %topic_name, "periodic commit skipped: {e}");
                }
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

                        let partition = borrowed_msg.partition();
                        let offset = borrowed_msg.offset();
                        // Register the offset as in flight before dispatch, so
                        // out-of-order completions cannot advance the commit
                        // watermark past it.
                        tracker.lock().unwrap_or_else(|e| e.into_inner()).on_receive(partition, offset);

                        let metadata = extract_metadata_from_kafka(&borrowed_msg);
                        // Returns once handler tasks are spawned (permit-bounded);
                        // the loop stays pipelined and keeps pulling messages.
                        let completion = inner
                            .state
                            .dispatch_from_poller_tracked(type_id, payload, metadata)
                            .await;

                        spawn_completion_forwarder(completion, (partition, offset), completion_tx.clone());
                    }
                    Err(e) => {
                        tracing::warn!(topic = %topic_name, "Kafka consumer error: {e}");
                        r2e_core::rt::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
            Some(((partition, offset), outcome)) = completion_rx.recv() => {
                apply_completion(&consumer, &tracker, topic_name, partition, offset, outcome);
            }
        }
    }

    // Drain pending completion decisions best-effort before dropping the
    // consumer. Dropping our sender lets `recv` return `None` once every
    // forwarder has reported; the timeout caps waiting on handlers still
    // running. Undrained completions just mean redelivery (at-least-once).
    drop(completion_tx);
    let drain = async {
        while let Some(((partition, offset), outcome)) = completion_rx.recv().await {
            apply_completion(&consumer, &tracker, topic_name, partition, offset, outcome);
        }
    };
    let _ = r2e_core::rt::timeout(COMPLETION_DRAIN_TIMEOUT, drain).await;

    // Final synchronous commit of everything stored during the drain, so a
    // clean shutdown does not needlessly redeliver acked messages. Only needed
    // when librdkafka's periodic committer is off.
    if manual_commit {
        if let Err(e) = consumer.commit_consumer_state(CommitMode::Sync) {
            tracing::debug!(topic = %topic_name, "final commit skipped: {e}");
        }
    }
}

/// Apply a single completion decision to the tracker and consumer.
///
/// On Ack, advances the partition watermark and stores the new offset (picked
/// up by librdkafka's periodic committer, or the loop's manual commit when
/// auto-commit is disabled). On Nack, pins the partition so nothing at or above
/// the failed offset commits — the message is redelivered on rebalance/restart.
/// The tracker mutex is only held for the tracker call, never across the store.
fn apply_completion(
    consumer: &StreamConsumer<R2eConsumerContext>,
    tracker: &std::sync::Mutex<WatermarkTracker<i32, i64>>,
    topic_name: &str,
    partition: i32,
    offset: i64,
    outcome: DispatchOutcome,
) {
    match outcome {
        DispatchOutcome::Ack => {
            let store = tracker.lock().unwrap_or_else(|e| e.into_inner()).on_ack(partition, offset);
            if let Some(store_offset) = store {
                // `store_offset` is the message offset; librdkafka commits `+1`.
                if let Err(e) = consumer.store_offset(topic_name, partition, store_offset) {
                    tracing::warn!(
                        topic = %topic_name,
                        partition,
                        offset = store_offset,
                        "failed to store Kafka offset: {e}"
                    );
                }
            }
        }
        DispatchOutcome::Nack => {
            tracker.lock().unwrap_or_else(|e| e.into_inner()).on_nack(partition, offset);
            tracing::warn!(
                topic = %topic_name,
                partition,
                offset,
                "handler nacked without DLQ capture — not committing offset; \
                 message will be redelivered on rebalance/reconnect"
            );
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
