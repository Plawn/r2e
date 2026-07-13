// NOTE: The `pulsar` client library is tokio-bound; any tokio APIs that
// originate from the pulsar SDK remain on direct tokio and are a documented
// exception to the r2e_core::rt facade.
use std::any::TypeId;
use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use pulsar::consumer::{Consumer, ConsumerOptions, InitialPosition};
use pulsar::message::proto::command_subscribe::SubType;
use pulsar::message::proto::MessageIdData;
use pulsar::producer::Message as ProducerMessage;
use pulsar::{TokioExecutor, Error as PulsarError};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use r2e_events::backend::{
    await_reply, decode_metadata, decode_reply_headers, encode_metadata, encode_reply_headers,
    reconnect_loop, reply_topic, request_topic, spawn_completion_forwarder, DispatchOutcome,
    Handler, COMPLETION_CHANNEL_CAPACITY, COMPLETION_DRAIN_TIMEOUT, HEADER_PARTITION_KEY,
};
use r2e_events::{
    EventBus, EventBusError, EventEnvelope, EventMetadata, HandlerResult, RequestOptions,
    ResponderHandle, SubscriptionHandle,
};

use crate::builder::PulsarEventBusBuilder;
use crate::config::PulsarConfig;
use crate::error::map_pulsar_error;
use crate::inner::PulsarInner;

/// Subscription name used by the per-instance reply consumer. The reply topic is
/// already unique per bus instance (it carries a per-instance nonce), so an
/// `Exclusive` subscription on it names a single consumer.
const REPLY_SUBSCRIPTION: &str = "r2e-reply";

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
/// - `emit` is fan-out publish/subscribe; use `request`/`respond` for
///   point-to-point request-reply.
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

    /// Get or create a cached per-topic producer handle.
    ///
    /// The map lock is taken only briefly to clone (fast path) or double-check
    /// and insert (slow path) the per-topic `Arc<Mutex<Producer>>`. The broker
    /// connect happens OUTSIDE the map lock so a first emit to a new topic does
    /// not block emits on other topics.
    async fn get_or_create_producer(
        &self,
        full_topic: &str,
    ) -> Result<Arc<tokio::sync::Mutex<pulsar::producer::Producer<TokioExecutor>>>, EventBusError> {
        // Fast path: return the existing handle under a brief map lock.
        {
            let producers = self.inner.producers.lock().await;
            if let Some(handle) = producers.get(full_topic) {
                return Ok(handle.clone());
            }
        }

        // Slow path: build the producer without holding the map lock.
        let producer = self
            .inner
            .pulsar
            .producer()
            .with_topic(full_topic)
            .build()
            .await
            .map_err(map_pulsar_error)?;

        // Double-checked insert: if another task raced us and inserted first,
        // keep theirs and drop ours (the un-inserted `producer` is dropped
        // cleanly when the closure that never ran goes out of scope).
        let mut producers = self.inner.producers.lock().await;
        let handle = producers
            .entry(full_topic.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(producer)))
            .clone();
        Ok(handle)
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

        // Resolve the per-topic producer handle (builds outside any map lock).
        let producer = self.get_or_create_producer(&full_topic).await?;

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

        // Lock only this topic's producer for the send; other topics proceed in
        // parallel. Release before awaiting the broker receipt.
        let receipt = {
            let mut guard = producer.lock().await;
            guard
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

    /// Publish a message to an already-fully-qualified topic with explicit
    /// properties (used by the request-reply paths, which carry reply control
    /// headers rather than plain event metadata).
    async fn publish_to_full(
        &self,
        full_topic: &str,
        payload: Vec<u8>,
        properties: HashMap<String, String>,
        partition_key: Option<String>,
    ) -> Result<(), EventBusError> {
        let producer = self.get_or_create_producer(full_topic).await?;

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

        let receipt = {
            let mut guard = producer.lock().await;
            guard
                .send_non_blocking(msg)
                .await
                .map_err(|e: PulsarError| map_pulsar_error(e))?
        };

        receipt
            .await
            .map_err(|e| EventBusError::Other(format!("send receipt error: {e}")))?;

        Ok(())
    }

    /// The fully-qualified, instance-private reply topic this bus consumes.
    ///
    /// Derived once at build time from a per-instance nonce (see
    /// [`PulsarInner::instance_id`]) and cached, so every request reuses the same
    /// string instead of re-deriving it. Two bus instances sharing a
    /// `config.subscription` in one process get distinct reply topics, so their
    /// `Exclusive` reply subscriptions never collide. The requester advertises it
    /// in the request's reply-to header; the reply consumer subscribes to it.
    fn reply_topic_full(&self) -> String {
        self.inner
            .reply_topic_full
            .get_or_init(|| {
                let short = reply_topic(&self.inner.config.subscription, self.inner.instance_id);
                self.full_topic(&short)
            })
            .clone()
    }

    /// Ensure the single per-instance reply consumer is running.
    ///
    /// Idempotent and started lazily on the first `request_with`: the first
    /// caller spawns the reconnecting reply-consumer task and records its
    /// cancellation token; later callers observe the already-set token and
    /// return immediately.
    fn ensure_reply_consumer(&self) {
        self.inner.reply_consumer.get_or_init(|| {
            let cancel = CancellationToken::new();
            let inner = self.inner.clone();
            let full_reply = self.reply_topic_full();
            let cancel_task = cancel.clone();
            r2e_core::rt::spawn(async move {
                run_reply_consumer(inner, full_reply, cancel_task).await;
            });
            cancel
        });
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

                r2e_core::rt::spawn(async move {
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

                r2e_core::rt::spawn(async move {
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

    fn request_with<Req, Resp>(
        &self,
        req: Req,
        options: RequestOptions,
    ) -> impl Future<Output = Result<Resp, EventBusError>> + Send
    where
        Req: Serialize + Send + Sync + 'static,
        Resp: DeserializeOwned + Send + 'static,
    {
        let bus = self.clone();
        async move {
            bus.inner.state.check_shutdown()?;

            // Lazily start the per-instance reply consumer (one for all types).
            bus.ensure_reply_consumer();

            let payload = serde_json::to_vec(&req)
                .map_err(|e| EventBusError::Serialization(e.to_string()))?;

            let resolved = bus.resolve_topic::<Req>();
            let full_request_topic = bus.full_topic(&request_topic(&resolved));
            let reply_to = bus.reply_topic_full();

            let metadata = options.metadata.unwrap_or_default();
            let partition_key = metadata.partition_key.clone();

            // Register the pending request and tag the message with its
            // request id + reply-to. The guard evicts the map entry when this
            // future returns (reply, timeout, or error).
            let (id, guard, rx) = bus.inner.pending.register();

            let mut properties: HashMap<String, String> =
                encode_metadata(&metadata).into_iter().collect();
            for (k, v) in encode_reply_headers(id, Some(&reply_to), None) {
                properties.insert(k, v);
            }

            bus.publish_to_full(&full_request_topic, payload, properties, partition_key)
                .await?;

            // Await the reply against the timeout and a shutdown signal. On
            // distributed backends an absent responder is indistinguishable from
            // a slow one, so it manifests as `RequestTimeout` (not `NoResponder`,
            // which only the in-process bus can detect synchronously). The reply
            // consumer's cancellation token (cancelled on `shutdown`) is the
            // shutdown future; the pending guard drops on every exit path so a
            // late reply is discarded instead of leaking a correlation-map slot.
            let cancel = bus.inner.reply_consumer.get().cloned();
            let result = await_reply::<Resp>(rx, options.timeout, async move {
                match cancel {
                    Some(token) => token.cancelled().await,
                    None => std::future::pending::<()>().await,
                }
            })
            .await;
            drop(guard);
            result
        }
    }

    fn respond<Req, Resp, F, Fut>(
        &self,
        handler: F,
    ) -> impl Future<Output = Result<ResponderHandle, EventBusError>> + Send
    where
        Req: DeserializeOwned + Send + Sync + 'static,
        Resp: Serialize + Send + 'static,
        F: Fn(EventEnvelope<Req>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Resp, String>> + Send + 'static,
    {
        let bus = self.clone();
        async move {
            bus.inner.state.check_shutdown()?;

            let type_id = TypeId::of::<Req>();
            let type_name = std::any::type_name::<Req>();

            // At most one responder per request type per process — errors on a
            // duplicate before any consumer is started.
            bus.inner
                .state
                .register_responder::<Req, Resp, F, Fut>(handler)
                .await?;

            // Consume the shared request topic with a `Shared` subscription so
            // the broker load-balances each request to exactly one instance.
            let resolved = bus.resolve_topic::<Req>();
            let full_request_topic = bus.full_topic(&request_topic(&resolved));

            let cancel = CancellationToken::new();
            bus.inner
                .responder_cancels
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(type_id, cancel.clone());

            let inner = bus.inner.clone();
            let cancel_task = cancel.clone();
            r2e_core::rt::spawn(async move {
                run_responder(inner, type_id, full_request_topic, cancel_task).await;
            });

            let inner_unreg = bus.inner.clone();
            Ok(ResponderHandle::new(type_name, move || {
                let inner = inner_unreg.clone();
                // Unregister may be triggered from a responder, so route to the
                // control plane in sharded mode.
                r2e_core::rt::spawn_ctl(async move {
                    inner.state.unregister_responder(type_id).await;
                    if let Some(cancel) = inner
                        .responder_cancels
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(&type_id)
                    {
                        cancel.cancel();
                    }
                });
            }))
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

            // Cancel all subscribe pollers.
            inner.state.cancel_all_pollers();

            // Stop the reply consumer and every responder (request-topic)
            // consumer. In-flight requests still awaiting a reply then resolve
            // via their per-request timeout — no new replies can arrive once the
            // reply consumer is cancelled.
            if let Some(cancel) = inner.reply_consumer.get() {
                cancel.cancel();
            }
            {
                let mut responders = inner
                    .responder_cancels
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                for (_type_id, cancel) in responders.drain() {
                    cancel.cancel();
                }
            }

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
    let label = format!("Pulsar poller [{full_topic}]");
    reconnect_loop(
        config.reconnect,
        config.reconnect_max_backoff,
        &cancel,
        &label,
        || run_poller_inner(&inner, type_id, &full_topic, &config, &cancel),
    )
    .await;
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

    // Per-message ack decisions flow back on this bounded channel. The consumer
    // is `!Sync` and its `ack`/`nack` take `&mut self`, so only the consume loop
    // may touch it — completion tasks send the decision here and the loop applies
    // it inline. The bound provides backpressure on ack throughput.
    let (ack_tx, mut ack_rx) = tokio::sync::mpsc::channel::<((String, MessageIdData), DispatchOutcome)>(
        COMPLETION_CHANNEL_CAPACITY,
    );

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::info!(topic = %full_topic, "poller cancelled");
                break;
            }
            // Apply an ack/nack decision once its handlers have finished. Acks are
            // per-message and order-independent in Pulsar, so no watermark tracking.
            Some(((msg_topic, msg_id), outcome)) = ack_rx.recv() => {
                apply_ack(&mut consumer, full_topic, &msg_topic, msg_id, outcome).await;
            }
            msg = futures_util::StreamExt::next(&mut consumer) => {
                match msg {
                    Some(Ok(received)) => {
                        let metadata = extract_metadata_from_message(&received);
                        let msg_topic = received.topic.clone();
                        let msg_id = received.message_id.id.clone();

                        // Stay pipelined: dispatch returns once handler tasks are
                        // spawned (permit-bounded), so we keep pulling messages.
                        let completion = inner
                            .state
                            .dispatch_from_poller_tracked(type_id, &received.payload.data, metadata)
                            .await;

                        // Ack the broker copy only AFTER local handlers complete
                        // (at-least-once). A forwarder task awaits the outcome and
                        // sends the decision back to the loop.
                        spawn_completion_forwarder(completion, (msg_topic, msg_id), ack_tx.clone());
                    }
                    Some(Err(e)) => {
                        tracing::warn!(topic = %full_topic, "consumer error: {e}");
                        r2e_core::rt::sleep(std::time::Duration::from_secs(1)).await;
                    }
                    None => {
                        tracing::info!(topic = %full_topic, "consumer stream ended");
                        break;
                    }
                }
            }
        }
    }

    // Drain pending ack decisions best-effort before dropping the consumer.
    // Unacked messages are redelivered (the at-least-once guarantee), so a
    // bounded drain suffices: dropping our own sender lets `recv` return `None`
    // once every completion task has reported, and the timeout caps how long we
    // wait on stragglers still running a handler.
    drop(ack_tx);
    let drain = async {
        while let Some(((msg_topic, msg_id), outcome)) = ack_rx.recv().await {
            apply_ack(&mut consumer, full_topic, &msg_topic, msg_id, outcome).await;
        }
    };
    let _ = r2e_core::rt::timeout(COMPLETION_DRAIN_TIMEOUT, drain).await;
}

/// Apply a single ack/nack decision to the consumer.
///
/// `msg_topic` is the message's origin topic (routes multi-topic consumers to
/// the right internal consumer); `full_topic` is used only for logging.
async fn apply_ack(
    consumer: &mut Consumer<Vec<u8>, TokioExecutor>,
    full_topic: &str,
    msg_topic: &str,
    msg_id: MessageIdData,
    outcome: DispatchOutcome,
) {
    match outcome {
        DispatchOutcome::Ack => {
            if let Err(e) = consumer.ack_with_id(msg_topic, msg_id).await {
                tracing::warn!(topic = %full_topic, "failed to ack message: {e}");
            }
        }
        DispatchOutcome::Nack => {
            // A handler failed without DLQ capture — negative-ack so Pulsar
            // redelivers after negativeAckRedeliveryDelay (at-least-once).
            if let Err(e) = consumer.nack_with_id(msg_topic, msg_id).await {
                tracing::warn!(topic = %full_topic, "failed to nack message: {e}");
            }
        }
    }
}

/// Collect a Pulsar message's properties (plus its partition key) into the
/// string key-value pairs the shared codec decodes.
fn message_property_pairs(
    message: &pulsar::consumer::Message<Vec<u8>>,
) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = Vec::new();

    for kv in &message.payload.metadata.properties {
        pairs.push((kv.key.clone(), kv.value.clone()));
    }

    if let Some(ref key) = message.payload.metadata.partition_key {
        if !key.is_empty() {
            pairs.push((HEADER_PARTITION_KEY.to_string(), key.clone()));
        }
    }

    pairs
}

/// Extract `EventMetadata` from Pulsar message properties.
fn extract_metadata_from_message(
    message: &pulsar::consumer::Message<Vec<u8>>,
) -> EventMetadata {
    decode_metadata(message_property_pairs(message).into_iter())
}

// ── Request-reply consumers ────────────────────────────────────────────────

/// Background consumer for a single request topic (responder side).
///
/// Reconnecting driver mirroring [`run_poller`]: consumes the shared
/// `<topic>.requests` topic with a `Shared` subscription so the broker delivers
/// each request to exactly one instance.
async fn run_responder(
    inner: Arc<PulsarInner>,
    type_id: TypeId,
    full_topic: String,
    cancel: CancellationToken,
) {
    let label = format!("Pulsar responder [{full_topic}]");
    reconnect_loop(
        inner.config.reconnect,
        inner.config.reconnect_max_backoff,
        &cancel,
        &label,
        || run_responder_inner(&inner, type_id, &full_topic, &cancel),
    )
    .await;
}

async fn run_responder_inner(
    inner: &Arc<PulsarInner>,
    type_id: TypeId,
    full_topic: &str,
    cancel: &CancellationToken,
) {
    let consumer_result: Result<Consumer<Vec<u8>, TokioExecutor>, PulsarError> = inner
        .pulsar
        .consumer()
        .with_topic(full_topic)
        .with_subscription(&inner.config.subscription)
        .with_subscription_type(SubType::Shared)
        .with_consumer_name(format!("r2e-responder-{full_topic}"))
        .build()
        .await;

    let mut consumer = match consumer_result {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(topic = %full_topic, "failed to create Pulsar responder consumer: {e}");
            return;
        }
    };

    tracing::info!(topic = %full_topic, "responder started");

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::info!(topic = %full_topic, "responder cancelled");
                break;
            }
            msg = futures_util::StreamExt::next(&mut consumer) => {
                match msg {
                    Some(Ok(received)) => {
                        // Ack the request only after its reply has actually been
                        // published (at-least-once). If the reply publish failed,
                        // negative-ack so the broker redelivers the request to
                        // another instance instead of losing it. An error REPLY
                        // that published successfully still acks.
                        if handle_request(inner, type_id, full_topic, &received).await {
                            if let Err(e) = consumer.ack(&received).await {
                                tracing::warn!(topic = %full_topic, "failed to ack request: {e}");
                            }
                        } else if let Err(e) = consumer.nack(&received).await {
                            tracing::warn!(topic = %full_topic, "failed to nack request: {e}");
                        }
                    }
                    Some(Err(e)) => {
                        tracing::warn!(topic = %full_topic, "responder consumer error: {e}");
                        r2e_core::rt::sleep(std::time::Duration::from_secs(1)).await;
                    }
                    None => {
                        tracing::info!(topic = %full_topic, "responder stream ended");
                        break;
                    }
                }
            }
        }
    }
}

/// Decode one request, invoke the responder, and publish the reply.
///
/// Returns whether the request should be acked: `true` when the reply was
/// published (including an error reply), or when the request is malformed and
/// can never be answered (no request id / no reply-to — redelivery would not
/// help). Returns `false` only when the reply publish itself failed, so the
/// caller negative-acks and the broker redelivers.
async fn handle_request(
    inner: &Arc<PulsarInner>,
    type_id: TypeId,
    full_topic: &str,
    received: &pulsar::consumer::Message<Vec<u8>>,
) -> bool {
    let pairs = message_property_pairs(received);
    let reply_headers = decode_reply_headers(pairs.iter().map(|(k, v)| (k, v)));

    let reply_headers = match reply_headers {
        Some(rh) => rh,
        None => {
            tracing::warn!(topic = %full_topic, "request without request id — dropping");
            return true;
        }
    };
    let reply_to = match reply_headers.reply_to {
        Some(ref rt) => rt.clone(),
        None => {
            tracing::warn!(topic = %full_topic, "request without reply-to — dropping");
            return true;
        }
    };

    let metadata = decode_metadata(pairs.iter().map(|(k, v)| (k, v)));

    // Build the reply payload + optional error (single-sourced mapping, incl.
    // the no-responder-registered error reply) rather than hand-rolling it.
    let (payload, error) = inner
        .state
        .build_reply(type_id, &received.payload.data, metadata)
        .await;

    let properties: HashMap<String, String> =
        encode_reply_headers(reply_headers.request_id, None, error.as_deref())
            .into_iter()
            .collect();

    // Reuse the per-topic producer cache to publish the reply to the requester's
    // instance-private reply topic (already fully qualified). Only ack the
    // request once this publish succeeds — otherwise the reply would be lost.
    let bus = PulsarEventBus { inner: inner.clone() };
    match bus.publish_to_full(&reply_to, payload, properties, None).await {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(reply_to = %reply_to, "failed to publish reply, redelivering request: {e}");
            false
        }
    }
}

/// Background consumer for this instance's private reply topic (requester side).
///
/// Reconnecting driver: routes each reply back to the waiting requester by
/// request id via [`PendingRequests::complete_reply`].
///
/// [`PendingRequests::complete_reply`]: r2e_events::backend::PendingRequests::complete_reply
async fn run_reply_consumer(
    inner: Arc<PulsarInner>,
    full_topic: String,
    cancel: CancellationToken,
) {
    let label = format!("Pulsar reply consumer [{full_topic}]");
    reconnect_loop(
        inner.config.reconnect,
        inner.config.reconnect_max_backoff,
        &cancel,
        &label,
        || run_reply_consumer_inner(&inner, &full_topic, &cancel),
    )
    .await;
}

async fn run_reply_consumer_inner(
    inner: &Arc<PulsarInner>,
    full_topic: &str,
    cancel: &CancellationToken,
) {
    let consumer_result: Result<Consumer<Vec<u8>, TokioExecutor>, PulsarError> = inner
        .pulsar
        .consumer()
        .with_topic(full_topic)
        .with_subscription(REPLY_SUBSCRIPTION)
        .with_subscription_type(SubType::Exclusive)
        // Start from the earliest message: a reply that arrives while this
        // subscription is still being established (e.g. during the very first
        // request) must not be skipped. Safe because the topic is
        // instance-private — the only messages here are our own replies. Mirrors
        // the Kafka reply consumer's `auto.offset.reset=earliest`.
        .with_options(ConsumerOptions::default().with_initial_position(InitialPosition::Earliest))
        .with_consumer_name(format!("r2e-reply-{full_topic}"))
        .build()
        .await;

    let mut consumer = match consumer_result {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(topic = %full_topic, "failed to create Pulsar reply consumer: {e}");
            return;
        }
    };

    tracing::info!(topic = %full_topic, "reply consumer started");

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::info!(topic = %full_topic, "reply consumer cancelled");
                break;
            }
            msg = futures_util::StreamExt::next(&mut consumer) => {
                match msg {
                    Some(Ok(received)) => {
                        let pairs = message_property_pairs(&received);
                        if let Some(rh) = decode_reply_headers(pairs.iter().map(|(k, v)| (k, v))) {
                            inner
                                .pending
                                .complete_reply(&rh, received.payload.data.clone());
                        } else {
                            tracing::warn!(topic = %full_topic, "reply without request id — dropping");
                        }
                        // Replies are terminal; ack unconditionally.
                        if let Err(e) = consumer.ack(&received).await {
                            tracing::warn!(topic = %full_topic, "failed to ack reply: {e}");
                        }
                    }
                    Some(Err(e)) => {
                        tracing::warn!(topic = %full_topic, "reply consumer error: {e}");
                        r2e_core::rt::sleep(std::time::Duration::from_secs(1)).await;
                    }
                    None => {
                        tracing::info!(topic = %full_topic, "reply consumer stream ended");
                        break;
                    }
                }
            }
        }
    }
}
