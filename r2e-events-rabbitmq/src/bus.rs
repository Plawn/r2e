use std::any::TypeId;
use std::future::Future;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use lapin::options::{
    BasicAckOptions, BasicConsumeOptions, BasicNackOptions, BasicPublishOptions,
    QueueBindOptions, QueueDeclareOptions,
};
use lapin::types::{AMQPValue, FieldTable, LongString, ShortString};
use lapin::BasicProperties;
use serde::de::DeserializeOwned;
use serde::Serialize;
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use r2e_events::backend::{decode_metadata, encode_metadata, Handler};
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
    async fn resolve_topic<E: 'static>(&self) -> String {
        self.inner.state.resolve_topic::<E>().await
    }

    /// Ensure a queue exists and is bound to the exchange for the given topic.
    async fn ensure_queue(&self, topic_name: &str) -> Result<String, EventBusError> {
        let queue_name = format!("{}.{}", self.inner.config.consumer_group, topic_name);

        if self.inner.state.is_topic_ensured(topic_name).await {
            return Ok(queue_name);
        }

        if !self.inner.config.auto_create {
            self.inner.state.set_topic_ensured(topic_name).await;
            return Ok(queue_name);
        }

        // Build queue arguments
        let mut args = FieldTable::default();

        if let Some(ttl) = self.inner.config.message_ttl_ms {
            args.insert(
                ShortString::from("x-message-ttl"),
                AMQPValue::LongUInt(ttl),
            );
        }

        if let Some(ref dlx) = self.inner.config.dead_letter_exchange {
            args.insert(
                ShortString::from("x-dead-letter-exchange"),
                AMQPValue::LongString(LongString::from(dlx.as_bytes())),
            );
        }

        // Declare queue
        self.inner
            .channel
            .queue_declare(
                &queue_name,
                QueueDeclareOptions {
                    durable: self.inner.config.durable,
                    ..QueueDeclareOptions::default()
                },
                args,
            )
            .await
            .map_err(map_lapin_error)?;

        // Bind queue to exchange with routing key = topic name
        self.inner
            .channel
            .queue_bind(
                &queue_name,
                &self.inner.config.exchange,
                topic_name,
                QueueBindOptions::default(),
                FieldTable::default(),
            )
            .await
            .map_err(map_lapin_error)?;

        tracing::info!(
            queue = %queue_name,
            exchange = %self.inner.config.exchange,
            routing_key = %topic_name,
            "declared and bound queue"
        );

        self.inner.state.set_topic_ensured(topic_name).await;
        Ok(queue_name)
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
    async fn publish(
        &self,
        topic_name: &str,
        payload: Vec<u8>,
        metadata: &EventMetadata,
    ) -> Result<(), EventBusError> {
        let props = self.build_properties(metadata);

        self.inner
            .channel
            .basic_publish(
                &self.inner.config.exchange,
                topic_name,
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
            inner.state.topic_registry.write().await.register_by_type_id(type_id, topic);
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
            let topic_name = bus.resolve_topic::<E>().await;

            let h: Handler = Arc::new(move |any, metadata| {
                let event = any.downcast::<E>().expect("event type mismatch");
                let envelope = EventEnvelope { event, metadata };
                Box::pin(handler(envelope))
            });

            let (id, is_first) = inner.state.register_handler::<E>(h).await;

            // If this is the first subscriber for this type, set up the consumer
            if is_first {
                let queue_name = bus.ensure_queue(&topic_name).await?;

                let cancel = CancellationToken::new();
                inner
                    .state
                    .poller_cancels
                    .lock()
                    .await
                    .insert(type_id, cancel.clone());

                let inner_clone = bus.inner.clone();
                let queue_clone = queue_name.clone();

                tokio::spawn(async move {
                    run_consumer(inner_clone, type_id, queue_clone, cancel).await;
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
            let topic_name = bus.resolve_topic::<E>().await;

            let h: Handler = Arc::new(move |any, metadata| {
                let event = any.downcast::<E>().expect("event type mismatch");
                let envelope = EventEnvelope { event, metadata };
                Box::pin(handler(envelope))
            });

            let (id, is_first) = inner.state.register_handler_with_deserializer::<E>(h, deserializer).await;

            if is_first {
                let queue_name = bus.ensure_queue(&topic_name).await?;

                let cancel = CancellationToken::new();
                inner
                    .state
                    .poller_cancels
                    .lock()
                    .await
                    .insert(type_id, cancel.clone());

                let inner_clone = bus.inner.clone();
                let queue_clone = queue_name.clone();

                tokio::spawn(async move {
                    run_consumer(inner_clone, type_id, queue_clone, cancel).await;
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
            bus.inner.state.check_shutdown()?;

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
            bus.inner.state.check_shutdown()?;

            let type_id = TypeId::of::<E>();
            let payload = serde_json::to_vec(&event)
                .map_err(|e| EventBusError::Serialization(e.to_string()))?;
            let topic_name = bus.resolve_topic::<E>().await;
            let metadata = EventMetadata::new();

            // Publish to RabbitMQ
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
            let topic_name = bus.resolve_topic::<E>().await;

            // Publish to RabbitMQ
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
            inner.state.cancel_all_pollers().await;
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
            inner.state.shutdown.store(true, Ordering::SeqCst);

            // Cancel all consumer tasks
            inner.state.cancel_all_pollers().await;

            // Wait for in-flight handlers to complete
            inner.state.wait_in_flight(timeout).await?;

            // Clear handlers
            inner.state.handlers.write().await.clear();

            // Close the channel gracefully
            if let Err(e) = inner.channel.close(200, "shutdown").await {
                tracing::warn!("error closing RabbitMQ channel: {e}");
            }

            Ok(())
        }
    }
}

/// Background consumer loop for a single queue/topic with automatic reconnection.
async fn run_consumer(
    inner: Arc<RabbitMqInner>,
    type_id: TypeId,
    queue_name: String,
    cancel: CancellationToken,
) {
    let max_backoff = inner.config.reconnect_max_backoff;
    let reconnect = inner.config.reconnect;
    let mut backoff = std::time::Duration::from_secs(1);

    loop {
        let start = std::time::Instant::now();
        run_consumer_inner(&inner, type_id, &queue_name, &cancel).await;

        if cancel.is_cancelled() || !reconnect {
            break;
        }

        // Reset backoff if the consumer ran successfully for a while
        if start.elapsed() > backoff * 4 {
            backoff = std::time::Duration::from_secs(1);
        }

        tracing::warn!(queue = %queue_name, "RabbitMQ consumer disconnected, reconnecting in {backoff:?}");
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(backoff) => {},
        }
        backoff = (backoff * 2).min(max_backoff);
    }
}

async fn run_consumer_inner(
    inner: &Arc<RabbitMqInner>,
    type_id: TypeId,
    queue_name: &str,
    cancel: &CancellationToken,
) {
    // Start consuming from the queue
    let consumer_tag = format!("r2e-{}", queue_name);
    let consumer = match inner
        .channel
        .basic_consume(
            queue_name,
            &consumer_tag,
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
                        let metadata = extract_metadata_from_delivery(&delivery);
                        let payload = delivery.data.as_slice();

                        // Skip if this event was already dispatched locally by emit_and_wait.
                        if inner.state.locally_dispatched.lock().await.remove(metadata.event_id) {
                            if let Err(e) = delivery.ack(BasicAckOptions::default()).await {
                                tracing::error!(queue = %queue_name, "failed to ack deduped delivery: {e}");
                            }
                            continue;
                        }

                        // Attempt to dispatch to local handlers
                        let map = inner.state.handlers.read().await;
                        let dispatch_ok = if let Some(topic_handlers) = map.get(&type_id) {
                            match (topic_handlers.deserializer)(payload) {
                                Ok(event) => {
                                    // Dispatch to all handlers, respecting filters + retry
                                    let mut tasks = Vec::new();
                                    for entry in &topic_handlers.entries {
                                        // Check filter
                                        if entry.filter.as_ref().is_some_and(|f| !f(&metadata)) {
                                            continue;
                                        }
                                        let h = entry.handler.clone();
                                        let e = event.clone();
                                        let m = metadata.clone();
                                        let retry_policy = entry.retry_policy.clone();

                                        inner.state.in_flight.fetch_add(1, Ordering::SeqCst);

                                        let state = inner.state.clone();
                                        tasks.push(tokio::spawn(async move {
                                            let result = if let Some(ref policy) = retry_policy {
                                                r2e_events::backend::BackendState::invoke_with_retry(&h, &e, &m, policy).await
                                            } else {
                                                h(e, m).await
                                            };
                                            if state.in_flight.fetch_sub(1, Ordering::SeqCst) == 1 {
                                                state.in_flight_zero.notify_waiters();
                                            }
                                            result
                                        }));
                                    }

                                    // Collect DLQ info before dropping map
                                    let dlq_payload = payload.to_vec();
                                    drop(map);

                                    // Wait for all handler tasks and check results
                                    let mut all_ack = true;
                                    for task in tasks {
                                        match task.await {
                                            Ok(HandlerResult::Ack) => {}
                                            Ok(HandlerResult::Nack(reason)) => {
                                                tracing::warn!(queue = %queue_name, "handler returned Nack: {reason}");
                                                all_ack = false;
                                            }
                                            Err(e) => {
                                                tracing::error!(queue = %queue_name, "handler task panicked: {e}");
                                                all_ack = false;
                                            }
                                        }
                                    }

                                    // Publish to DLQ on final failure if configured
                                    if !all_ack {
                                        if let Some(ref publisher) = inner.state.dlq_publisher {
                                            // Re-read handlers to find DLQ topics
                                            let map = inner.state.handlers.read().await;
                                            if let Some(th) = map.get(&type_id) {
                                                for entry in &th.entries {
                                                    if let Some(ref policy) = entry.retry_policy {
                                                        if let Some(ref dlq_topic) = policy.dead_letter_topic {
                                                            publisher(dlq_topic.clone(), dlq_payload.clone(), metadata.clone()).await;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    all_ack
                                }
                                Err(err) => {
                                    drop(map);
                                    tracing::error!(queue = %queue_name, "failed to deserialize event: {err}");
                                    false
                                }
                            }
                        } else {
                            drop(map);
                            true // No handlers — ack anyway
                        };

                        // Ack or nack the delivery
                        if dispatch_ok {
                            if let Err(e) = delivery.ack(BasicAckOptions::default()).await {
                                tracing::error!(queue = %queue_name, "failed to ack delivery: {e}");
                            }
                        } else {
                            if let Err(e) = delivery
                                .nack(BasicNackOptions {
                                    requeue: true,
                                    ..BasicNackOptions::default()
                                })
                                .await
                            {
                                tracing::error!(queue = %queue_name, "failed to nack delivery: {e}");
                            }
                        }
                    }
                    Some(Err(e)) => {
                        tracing::warn!(queue = %queue_name, "consumer error: {e}");
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
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
