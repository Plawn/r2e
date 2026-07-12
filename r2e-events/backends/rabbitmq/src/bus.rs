// NOTE: The `lapin` (AMQP) client library is tokio-bound; any tokio APIs that
// originate from the lapin SDK remain on direct tokio and are a documented
// exception to the r2e_core::rt facade.
use std::any::TypeId;
use std::future::Future;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use lapin::options::{
    BasicAckOptions, BasicConsumeOptions, BasicNackOptions, BasicPublishOptions,
};
use lapin::types::{AMQPValue, FieldTable, LongString, ShortString};
use lapin::{BasicProperties, Channel};
use serde::de::DeserializeOwned;
use serde::Serialize;
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use r2e_events::backend::{decode_metadata, encode_metadata, DispatchOutcome, Handler};
use r2e_events::{
    EventBus, EventBusError, EventEnvelope, EventMetadata, HandlerResult, SubscriptionHandle,
};

use crate::builder::RabbitMqEventBusBuilder;
use crate::config::RabbitMqConfig;
use crate::error::map_lapin_error;
use crate::inner::RabbitMqInner;

/// RabbitMQ-backed event bus using AMQP 0-9-1.
///
/// Publishes events as JSON messages to a topic exchange and consumes them
/// via dedicated queues per consumer group + topic combination.
///
/// `RabbitMqEventBus` is `Clone` — all clones share the same underlying
/// connection, channel, and handler registry.
///
/// # AMQP Model Mapping
///
/// - Event bus = Topic exchange (configurable name, default `"r2e-events"`)
/// - Event type = Routing key (the resolved topic name)
/// - Consumer group = Queue prefix (`{consumer_group}.{topic_name}`)
/// - Competing consumers = Multiple instances consuming the same queue
/// - Metadata = AMQP message headers
///
/// # Limitations
///
/// - `emit_and_wait` publishes to RabbitMQ AND waits for **local** handlers only.
///   It cannot wait for handlers on remote instances.
/// - RabbitMQ has no native partitioning. `partition_key` is stored as an AMQP
///   header but does not affect routing.
#[derive(Clone)]
pub struct RabbitMqEventBus {
    pub(crate) inner: Arc<RabbitMqInner>,
}

impl RabbitMqEventBus {
    /// Create a builder for configuring and connecting a `RabbitMqEventBus`.
    pub fn builder(config: RabbitMqConfig) -> RabbitMqEventBusBuilder {
        RabbitMqEventBusBuilder::new(config)
    }

    /// Resolve the topic name for an event type.
    fn resolve_topic<E: 'static>(&self) -> String {
        self.inner.state.resolve_topic::<E>()
    }

    /// Build AMQP BasicProperties from EventMetadata.
    fn build_properties(&self, metadata: &EventMetadata) -> BasicProperties {
        let pairs = encode_metadata(metadata);
        let mut headers = FieldTable::default();

        for (k, v) in pairs {
            headers.insert(
                ShortString::from(k),
                AMQPValue::LongString(LongString::from(v.as_bytes())),
            );
        }

        let mut props = BasicProperties::default()
            .with_content_type(ShortString::from("application/json"))
            .with_headers(headers);

        if self.inner.config.persistent {
            props = props.with_delivery_mode(2); // persistent
        }

        props
    }

    /// Publish a serialized event to RabbitMQ.
    ///
    /// Uses the dedicated publisher channel, recreating it (and reconnecting the
    /// underlying connection if needed) when the stored one has dropped. A
    /// publish failure only affects the publisher channel — consumer channels
    /// are independent.
    async fn publish(
        &self,
        topic_name: &str,
        payload: Vec<u8>,
        metadata: &EventMetadata,
    ) -> Result<(), EventBusError> {
        let props = self.build_properties(metadata);
        let channel = self.inner.publisher_channel().await?;

        channel
            .basic_publish(
                self.inner.config.exchange.as_str().into(),
                topic_name.into(),
                BasicPublishOptions::default(),
                &payload,
                props,
            )
            .await
            .map_err(map_lapin_error)?
            .await
            .map_err(map_lapin_error)?;

        Ok(())
    }
}

impl EventBus for RabbitMqEventBus {
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

            // If this is the first subscriber for this type, set up the consumer.
            if is_first {
                let (channel, queue_name) = setup_consumer_queue(&inner, &topic_name).await?;

                let cancel = inner.state.register_poller_cancel(type_id);

                let inner_clone = bus.inner.clone();

                r2e_core::rt::spawn(async move {
                    run_consumer(inner_clone, type_id, topic_name, cancel, Some((channel, queue_name))).await;
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
                let (channel, queue_name) = setup_consumer_queue(&inner, &topic_name).await?;

                let cancel = inner.state.register_poller_cancel(type_id);

                let inner_clone = bus.inner.clone();

                r2e_core::rt::spawn(async move {
                    run_consumer(inner_clone, type_id, topic_name, cancel, Some((channel, queue_name))).await;
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

            // Dispatch locally FIRST: this records the local outcome for the
            // poller dedup. Publishing before recording would race the poller
            // consuming the broker copy. If the local dispatch errors, skip the
            // publish entirely.
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

            // Dispatch locally FIRST (see `emit_and_wait`): records the local
            // outcome for the poller dedup before the broker copy can be seen.
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
        timeout: std::time::Duration,
    ) -> impl Future<Output = Result<(), EventBusError>> + Send {
        let inner = self.inner.clone();
        async move {
            // Set shutdown flag
            inner.state.shutdown.store(true, Ordering::Release);

            // Cancel all consumer tasks
            inner.state.cancel_all_pollers();

            // Wait for in-flight handlers to complete
            inner.state.wait_in_flight(timeout).await?;

            // Clear handlers
            inner.state.handlers.write().await.clear();

            // Close the connection gracefully (closes all channels with it).
            inner.close().await;

            Ok(())
        }
    }
}

/// Create a dedicated consumer channel and declare+bind its queue.
///
/// Called synchronously from `subscribe` for the first subscriber of a type so
/// that (a) the queue binding exists before `subscribe` returns — an `emit`
/// issued immediately afterwards can no longer be dropped by the topic exchange
/// — and (b) a declare failure (e.g. `PRECONDITION_FAILED` on conflicting queue
/// props) propagates out of `subscribe` instead of being swallowed by the
/// background task. The live channel is handed to `run_consumer` for its first
/// iteration rather than declared-and-dropped: dropping the channel would delete
/// an exclusive/auto-delete queue before the consumer could attach.
async fn setup_consumer_queue(
    inner: &Arc<RabbitMqInner>,
    topic_name: &str,
) -> Result<(Channel, String), EventBusError> {
    let channel = inner.new_consumer_channel().await?;
    let queue_name = inner.ensure_queue(&channel, topic_name).await?;
    Ok((channel, queue_name))
}

/// Background consumer loop for a single topic with automatic reconnection.
///
/// The first iteration consumes `initial` — the channel+queue already declared
/// and bound synchronously by `subscribe` — so the binding is guaranteed live
/// before the subscriber returned. Each subsequent (reconnect) iteration creates
/// a fresh dedicated channel (reconnecting the shared connection if the broker
/// link dropped), re-declares the queue and starts a new `basic_consume`. When
/// the consume stream ends or errors — the signal of a dropped channel/connection
/// — the loop backs off and reconnects.
async fn run_consumer(
    inner: Arc<RabbitMqInner>,
    type_id: TypeId,
    topic_name: String,
    cancel: CancellationToken,
    mut initial: Option<(Channel, String)>,
) {
    let max_backoff = inner.config.reconnect_max_backoff;
    let reconnect = inner.config.reconnect;
    let mut backoff = std::time::Duration::from_secs(1);

    loop {
        let start = std::time::Instant::now();
        // `initial` is consumed on the first pass; reconnect passes get `None`
        // and rebuild their own channel/queue.
        run_consumer_inner(&inner, type_id, &topic_name, &cancel, initial.take()).await;

        if cancel.is_cancelled() || !reconnect {
            break;
        }

        // Reset backoff if the consumer ran successfully for a while
        if start.elapsed() > backoff * 4 {
            backoff = std::time::Duration::from_secs(1);
        }

        tracing::warn!(topic = %topic_name, "RabbitMQ consumer disconnected, reconnecting in {backoff:?}");
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = r2e_core::rt::sleep(backoff) => {},
        }
        backoff = (backoff * 2).min(max_backoff);
    }
}

async fn run_consumer_inner(
    inner: &Arc<RabbitMqInner>,
    type_id: TypeId,
    topic_name: &str,
    cancel: &CancellationToken,
    initial: Option<(Channel, String)>,
) {
    // The channel is kept alive for the whole function; dropping it on return
    // closes only this consumer's channel.
    let (channel, queue_name) = match initial {
        // First iteration: reuse the channel `subscribe` already declared+bound.
        Some(ready) => ready,
        // Reconnect iteration: create a fresh dedicated channel (reconnecting
        // the shared connection if it dropped) and re-declare/bind the queue.
        None => {
            let channel = match inner.new_consumer_channel().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(topic = %topic_name, "failed to create consumer channel: {e}");
                    return;
                }
            };
            let queue_name = match inner.ensure_queue(&channel, topic_name).await {
                Ok(q) => q,
                Err(e) => {
                    tracing::error!(topic = %topic_name, "failed to declare queue: {e}");
                    return;
                }
            };
            (channel, queue_name)
        }
    };

    // Start consuming from the queue
    let consumer_tag = format!("r2e-{}", queue_name);
    let consumer = match channel
        .basic_consume(
            queue_name.as_str().into(),
            consumer_tag.as_str().into(),
            BasicConsumeOptions::default(),
            FieldTable::default(),
        )
        .await
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(queue = %queue_name, "failed to start consumer: {e}");
            return;
        }
    };

    tracing::info!(queue = %queue_name, "consumer started");

    // Shared owned handle for logging inside per-delivery acker tasks (cheap to
    // clone; avoids a String allocation per delivery).
    let queue: Arc<str> = Arc::from(queue_name.as_str());

    let mut consumer = consumer;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::info!(queue = %queue_name, "consumer cancelled");
                break;
            }
            delivery = consumer.next() => {
                match delivery {
                    Some(Ok(delivery)) => {
                        // Delegate all at-least-once logic to the shared engine:
                        // dedup, deserialization, handler collection, retry, DLQ
                        // capture, poison-message handling, backpressure and
                        // panic-safety. The returned completion resolves once all
                        // handlers finish, so the loop stays pipelined.
                        let metadata = extract_metadata_from_delivery(&delivery);
                        let completion = inner
                            .state
                            .dispatch_from_poller_tracked(type_id, &delivery.data, metadata)
                            .await;

                        // lapin's `Acker` is an owned, per-delivery handle usable
                        // from any task, so spawn a small follow-up task to ack/nack
                        // without serializing the poll loop. DLQ-captured and poison
                        // messages resolve as Ack in the shared engine, so
                        // `requeue: true` on Nack can only redeliver genuinely
                        // failed (retryable) messages, never poison payloads.
                        let acker = delivery.acker;
                        let queue = queue.clone();
                        r2e_core::rt::spawn(async move {
                            match completion.outcome().await {
                                DispatchOutcome::Ack => {
                                    if let Err(e) = acker.ack(BasicAckOptions::default()).await {
                                        tracing::error!(queue = %queue, "failed to ack delivery: {e}");
                                    }
                                }
                                DispatchOutcome::Nack => {
                                    tracing::warn!(queue = %queue, "dispatch nacked, requeueing delivery");
                                    if let Err(e) = acker
                                        .nack(BasicNackOptions {
                                            requeue: true,
                                            ..BasicNackOptions::default()
                                        })
                                        .await
                                    {
                                        tracing::error!(queue = %queue, "failed to nack delivery: {e}");
                                    }
                                }
                            }
                        });
                    }
                    Some(Err(e)) => {
                        // A stream-level error means the channel/connection has
                        // dropped: break so the outer loop reconnects with
                        // backoff (previously this slept and spun on the dead
                        // channel forever — the P2.2 bug).
                        tracing::warn!(queue = %queue_name, "consumer error, reconnecting: {e}");
                        break;
                    }
                    None => {
                        tracing::info!(queue = %queue_name, "consumer stream ended");
                        break;
                    }
                }
            }
        }
    }
}

/// Extract `EventMetadata` from AMQP delivery headers.
fn extract_metadata_from_delivery(delivery: &lapin::message::Delivery) -> EventMetadata {
    let mut pairs: Vec<(String, String)> = Vec::new();

    if let Some(headers) = delivery.properties.headers().as_ref() {
        for (key, value) in headers.inner() {
            let key_str = key.as_str();
            let val_str = match value {
                AMQPValue::LongString(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
                AMQPValue::ShortString(s) => s.to_string(),
                other => format!("{:?}", other),
            };
            pairs.push((key_str.to_string(), val_str));
        }
    }

    decode_metadata(pairs.into_iter())
}
