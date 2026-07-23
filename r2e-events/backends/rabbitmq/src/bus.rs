// NOTE: The `lapin` (AMQP) client library is tokio-bound; any tokio APIs that
// originate from the lapin SDK remain on direct tokio and are a documented
// exception to the r2e_core::rt facade.
use std::any::TypeId;
use std::future::Future;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use futures_util::StreamExt;
use lapin::options::{BasicAckOptions, BasicConsumeOptions, BasicNackOptions, BasicPublishOptions};
use lapin::types::{AMQPValue, FieldTable, LongString, ShortString};
use lapin::{BasicProperties, Channel};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use r2e_events::backend::{
    await_reply, decode_metadata, encode_metadata, reconnect_loop, request_topic, DispatchOutcome,
    Handler, HEADER_REPLY_ERROR,
};
use r2e_events::{
    EmitReceipt, EventBus, EventBusError, EventEnvelope, EventMetadata, HandlerResult,
    RequestOptions, ResponderHandle, SubscriptionHandle,
};

use crate::builder::RabbitMqEventBusBuilder;
use crate::config::RabbitMqConfig;
use crate::error::{map_lapin_error, require_publisher_ack};
use crate::inner::{RabbitMqInner, DIRECT_REPLY_TO};

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
/// - `emit` is fan-out publish/subscribe; use `request`/`respond` for
///   point-to-point request-reply.
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
    fn resolve_topic<E: 'static>(&self) -> Arc<str> {
        self.inner.state.resolve_topic::<E>()
    }

    /// Build AMQP BasicProperties from EventMetadata.
    fn build_properties(&self, metadata: &EventMetadata) -> BasicProperties {
        let pairs = encode_metadata(metadata);
        let mut headers = FieldTable::default();

        for (k, v) in pairs {
            headers.insert(
                ShortString::from(k.as_ref()),
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

    /// Build the AMQP properties for a request-reply request.
    ///
    /// Carries the event metadata as headers (like a normal publish) plus the
    /// Direct Reply-To address (`reply_to = amq.rabbitmq.reply-to`) and the
    /// pending `correlation_id`, so the responder knows where to send the reply
    /// and the requester's reply consumer can match it.
    fn build_request_properties(
        &self,
        metadata: &EventMetadata,
        correlation_id: u128,
    ) -> BasicProperties {
        self.build_properties(metadata)
            .with_reply_to(ShortString::from(DIRECT_REPLY_TO))
            .with_correlation_id(ShortString::from(correlation_id.to_string()))
    }

    /// Publish a serialized event to RabbitMQ.
    ///
    /// Uses the dedicated publisher channel, recreating it (and reconnecting the
    /// underlying connection if needed) when the stored one has dropped. A
    /// publish failure only affects the publisher channel — consumer channels
    /// are independent. Awaits the publisher confirm for durability.
    pub(crate) async fn publish(
        &self,
        topic_name: &str,
        payload: Vec<u8>,
        metadata: &EventMetadata,
    ) -> Result<(), EventBusError> {
        let props = self.build_properties(metadata);
        let channel = self.inner.publisher_channel().await?;

        let confirmation = channel
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
        require_publisher_ack(confirmation)
    }

    /// Publish without awaiting the publisher confirm. Returns an
    /// [`EmitReceipt`] wrapping the confirm future.
    async fn publish_nowait(
        &self,
        topic_name: &str,
        payload: Vec<u8>,
        metadata: &EventMetadata,
    ) -> Result<EmitReceipt, EventBusError> {
        let props = self.build_properties(metadata);
        let channel = self.inner.publisher_channel().await?;

        let confirm = channel
            .basic_publish(
                self.inner.config.exchange.as_str().into(),
                topic_name.into(),
                BasicPublishOptions::default(),
                &payload,
                props,
            )
            .await
            .map_err(map_lapin_error)?;

        Ok(EmitReceipt::new(async move {
            let confirmation = confirm.await.map_err(map_lapin_error)?;
            require_publisher_ack(confirmation)
        }))
    }
}

impl EventBus for RabbitMqEventBus {
    fn register_topic<E: 'static>(&self, topic: &str) -> impl Future<Output = ()> + Send {
        let inner = self.inner.clone();
        let topic = topic.to_string();
        async move {
            let type_id = TypeId::of::<E>();
            inner
                .state
                .topic_registry
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .register_by_type_id(type_id, topic);
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
            inner
                .state
                .configure_handler(handler_id.0, filter, retry_policy, Some(TypeId::of::<E>()))
                .await;
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
                let envelope = EventEnvelope {
                    event,
                    metadata: std::sync::Arc::new(metadata),
                };
                Box::pin(handler(envelope))
            });

            let (id, is_first) = inner.state.register_handler::<E>(h).await;

            // If this is the first subscriber for this type, set up the consumer.
            if is_first {
                let (channel, queue_name) = match setup_consumer_queue(&inner, &topic_name).await {
                    Ok(ready) => ready,
                    Err(error) => {
                        inner.state.unregister_handler(type_id, id).await;
                        return Err(error);
                    }
                };

                let cancel = inner.state.register_poller_cancel(type_id);

                let inner_clone = bus.inner.clone();

                r2e_core::rt::spawn(async move {
                    run_consumer(
                        inner_clone,
                        type_id,
                        topic_name,
                        cancel,
                        Some((channel, queue_name)),
                    )
                    .await;
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
                let envelope = EventEnvelope {
                    event,
                    metadata: std::sync::Arc::new(metadata),
                };
                Box::pin(handler(envelope))
            });

            let (id, is_first) = inner
                .state
                .register_handler_with_deserializer::<E>(h, deserializer)
                .await;

            if is_first {
                let (channel, queue_name) = match setup_consumer_queue(&inner, &topic_name).await {
                    Ok(ready) => ready,
                    Err(error) => {
                        inner.state.unregister_handler(type_id, id).await;
                        return Err(error);
                    }
                };

                let cancel = inner.state.register_poller_cancel(type_id);

                let inner_clone = bus.inner.clone();

                r2e_core::rt::spawn(async move {
                    run_consumer(
                        inner_clone,
                        type_id,
                        topic_name,
                        cancel,
                        Some((channel, queue_name)),
                    )
                    .await;
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

    fn emit_nowait<E>(
        &self,
        event: E,
    ) -> impl Future<Output = Result<EmitReceipt, EventBusError>> + Send
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
            bus.publish_nowait(&topic_name, payload, &metadata).await
        }
    }

    fn emit_nowait_with<E>(
        &self,
        event: E,
        metadata: EventMetadata,
    ) -> impl Future<Output = Result<EmitReceipt, EventBusError>> + Send
    where
        E: Serialize + Send + Sync + 'static,
    {
        let bus = self.clone();
        async move {
            bus.inner.state.check_shutdown()?;

            let payload = serde_json::to_vec(&event)
                .map_err(|e| EventBusError::Serialization(e.to_string()))?;
            let topic_name = bus.resolve_topic::<E>();
            bus.publish_nowait(&topic_name, payload, &metadata).await
        }
    }

    /// Send a point-to-point request via classic AMQP RPC (Direct Reply-To).
    ///
    /// The request is published to the topic exchange with routing key
    /// `<topic>.requests` (the shared request queue's binding) carrying
    /// `reply_to = amq.rabbitmq.reply-to` and a correlation id; the single
    /// responder's reply is routed back onto this process's requester channel
    /// and matched by correlation id.
    ///
    /// On distributed backends there is no way to distinguish "no responder is
    /// registered anywhere" from "the responder is slow": with no consumer on
    /// the shared request queue the request simply sits unconsumed, so an absent
    /// responder manifests as [`EventBusError::RequestTimeout`], not
    /// [`EventBusError::NoResponder`]. A responder that returns an error surfaces
    /// as [`EventBusError::Remote`].
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

            let payload = serde_json::to_vec(&req)
                .map_err(|e| EventBusError::Serialization(e.to_string()))?;
            let request_topic = request_topic(&bus.resolve_topic::<Req>());
            let metadata = options.metadata.unwrap_or_default();

            // Register the pending entry BEFORE publishing so a fast reply can
            // never race ahead of the correlation map. The guard evicts the entry
            // on drop (timeout / shutdown / early return).
            let (id, _guard, rx) = bus.inner.pending.register();
            let props = bus.build_request_properties(&metadata, id);

            // Publish on the requester channel — the same channel its Direct
            // Reply-To consumer runs on (an AMQP requirement).
            let channel = bus.inner.requester_channel().await?;
            let confirmation = channel
                .basic_publish(
                    bus.inner.config.exchange.as_str().into(),
                    request_topic.as_str().into(),
                    BasicPublishOptions::default(),
                    &payload,
                    props,
                )
                .await
                .map_err(map_lapin_error)?
                .await
                .map_err(map_lapin_error)?;
            require_publisher_ack(confirmation)?;

            // Shared request tail: races the reply against the timeout and the
            // per-bus shutdown token (`request_cancel`, mapped to
            // `EventBusError::Shutdown`). `_guard` stays alive across this await,
            // evicting the pending entry on return so a late reply is discarded.
            let cancel = bus.inner.request_cancel.clone();
            await_reply::<Resp>(rx, options.timeout, cancel.cancelled()).await
        }
    }

    /// Register the single responder for request type `Req` (classic AMQP RPC).
    ///
    /// Starts a consumer on the shared, `consumer_group`-independent request
    /// queue `<topic>.requests`. Every instance consumes that one queue, so the
    /// broker load-balances requests across instances, delivering each to exactly
    /// one responder. Each request is dispatched to `handler`, the reply is
    /// published to the request's Direct Reply-To address with the same
    /// correlation id, and the request is acked only after the reply is published
    /// (at-least-once; a duplicate reply is dropped by the requester once its
    /// correlation entry is gone). At most one responder per request type per
    /// process — a second registration returns an error out of this call.
    fn respond<Req, Resp, E, F, Fut>(
        &self,
        handler: F,
    ) -> impl Future<Output = Result<ResponderHandle, EventBusError>> + Send
    where
        Req: DeserializeOwned + Send + Sync + 'static,
        Resp: Serialize + Send + 'static,
        E: std::fmt::Display + Send + 'static,
        F: Fn(EventEnvelope<Req>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Resp, E>> + Send + 'static,
    {
        let bus = self.clone();
        async move {
            bus.inner.state.check_shutdown()?;

            let type_id = TypeId::of::<Req>();
            let type_name = std::any::type_name::<Req>();

            // Register the responder first — surfaces the "already registered"
            // error (one responder per type per process) before we touch the
            // broker.
            bus.inner
                .state
                .register_responder::<Req, Resp, E, F, Fut>(handler)
                .await?;

            let request_topic = request_topic(&bus.resolve_topic::<Req>());

            // Declare + bind the shared request queue synchronously so the
            // binding is live before `respond` returns and a declare failure
            // propagates out instead of being swallowed by the background task.
            // Unregister the responder if this setup fails.
            let setup = async {
                let channel = bus.inner.new_consumer_channel().await?;
                let queue_name = bus
                    .inner
                    .ensure_request_queue(&channel, &request_topic)
                    .await?;
                Ok::<_, EventBusError>((channel, queue_name))
            }
            .await;

            let (channel, queue_name) = match setup {
                Ok(ready) => ready,
                Err(e) => {
                    bus.inner.state.unregister_responder(type_id).await;
                    return Err(e);
                }
            };

            let cancel = bus.inner.state.register_poller_cancel(type_id);

            let inner_clone = bus.inner.clone();
            let cancel_task = cancel.clone();
            let topic_task = request_topic.clone();
            r2e_core::rt::spawn(async move {
                run_responder(
                    inner_clone,
                    type_id,
                    topic_task,
                    cancel_task,
                    Some((channel, queue_name)),
                )
                .await;
            });

            let inner_unreg = bus.inner.clone();
            Ok(ResponderHandle::new(type_name, move || {
                // Stop the request consumer and drop the responder.
                cancel.cancel();
                let inner = inner_unreg.clone();
                r2e_core::rt::spawn_ctl(async move {
                    inner.state.unregister_responder(type_id).await;
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

            // Cancel all consumer tasks (event pollers + responder request
            // consumers, both registered via `register_poller_cancel`).
            inner.state.cancel_all_pollers();

            // Fail in-flight requesters promptly rather than making them wait
            // out their timeouts; each drops its pending entry on return.
            inner.request_cancel.cancel();

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
    topic_name: Arc<str>,
    cancel: CancellationToken,
    mut initial: Option<(Channel, String)>,
) {
    let max_backoff = inner.config.reconnect_max_backoff;
    let reconnect = inner.config.reconnect;
    let label = format!("RabbitMQ consumer [{topic_name}]");

    // `initial.take()` is consumed on the first attempt; reconnect attempts get
    // `None` (the shared driver takes an `FnMut`, so the captured `Option` is
    // preserved across attempts) and rebuild their own channel/queue.
    reconnect_loop(reconnect, max_backoff, &cancel, &label, || {
        run_consumer_inner(&inner, type_id, &topic_name, &cancel, initial.take())
    })
    .await;
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

/// Background responder loop for one request type, with automatic reconnection.
///
/// Mirrors [`run_consumer`]'s reconnect skeleton (backoff, cancel-aware select,
/// first-iteration channel reuse) but dispatches each request to the registered
/// responder and publishes the reply, rather than fanning out to subscribers.
async fn run_responder(
    inner: Arc<RabbitMqInner>,
    type_id: TypeId,
    request_topic: String,
    cancel: CancellationToken,
    mut initial: Option<(Channel, String)>,
) {
    let max_backoff = inner.config.reconnect_max_backoff;
    let reconnect = inner.config.reconnect;
    let label = format!("RabbitMQ responder [{request_topic}]");

    // First attempt consumes `initial.take()` (the channel+queue `respond`
    // already declared+bound); reconnect attempts rebuild their own.
    reconnect_loop(reconnect, max_backoff, &cancel, &label, || {
        run_responder_inner(&inner, type_id, &request_topic, &cancel, initial.take())
    })
    .await;
}

async fn run_responder_inner(
    inner: &Arc<RabbitMqInner>,
    type_id: TypeId,
    request_topic: &str,
    cancel: &CancellationToken,
    initial: Option<(Channel, String)>,
) {
    let (channel, queue_name) = match initial {
        Some(ready) => ready,
        None => {
            let channel = match inner.new_consumer_channel().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(topic = %request_topic, "failed to create responder channel: {e}");
                    return;
                }
            };
            let queue_name = match inner.ensure_request_queue(&channel, request_topic).await {
                Ok(q) => q,
                Err(e) => {
                    tracing::error!(topic = %request_topic, "failed to declare request queue: {e}");
                    return;
                }
            };
            (channel, queue_name)
        }
    };

    let consumer_tag = format!("r2e-responder-{queue_name}");
    let mut consumer = match channel
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
            tracing::error!(queue = %queue_name, "failed to start responder consumer: {e}");
            return;
        }
    };

    tracing::info!(queue = %queue_name, "responder started");

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::info!(queue = %queue_name, "responder cancelled");
                break;
            }
            delivery = consumer.next() => {
                match delivery {
                    Some(Ok(delivery)) => {
                        // Invoke the responder, publish the reply to the request's
                        // Direct Reply-To address, then ack — off the poll loop so
                        // requests pipeline. `reply_to` is the broker-rewritten
                        // pseudo-queue; publish it via the default exchange.
                        let metadata = extract_metadata_from_delivery(&delivery);
                        let reply_to = delivery
                            .properties
                            .reply_to()
                            .as_ref()
                            .map(|s| s.to_string());
                        let correlation_id = delivery.properties.correlation_id().clone();
                        let payload = delivery.data;
                        let acker = delivery.acker;
                        let inner = inner.clone();
                        let channel = channel.clone();
                        let queue = queue_name.clone();
                        r2e_core::rt::spawn(async move {
                            // Shared outcome mapping: Ok reply bytes / responder
                            // error / (mid-flight-unregistered) no-responder all
                            // collapse to `(body, error)` with the shared wording.
                            let (body, error) = inner
                                .state
                                .build_reply(type_id, &payload, metadata)
                                .await;

                            // Track a failed reply publish so we can nack+requeue
                            // instead of losing the request (at-least-once).
                            let mut reply_publish_failed = false;
                            if let Some(reply_to) = reply_to {
                                let mut props = BasicProperties::default()
                                    .with_content_type(ShortString::from("application/json"));
                                if let Some(cid) = correlation_id {
                                    props = props.with_correlation_id(cid);
                                }
                                if let Some(msg) = &error {
                                    let mut headers = FieldTable::default();
                                    headers.insert(
                                        ShortString::from(HEADER_REPLY_ERROR),
                                        AMQPValue::LongString(LongString::from(msg.as_bytes())),
                                    );
                                    props = props.with_headers(headers);
                                }

                                match channel
                                    .basic_publish(
                                        "".into(),
                                        reply_to.as_str().into(),
                                        BasicPublishOptions::default(),
                                        &body,
                                        props,
                                    )
                                    .await
                                {
                                    Err(e) => {
                                        tracing::error!(queue = %queue, "failed to publish reply: {e}");
                                        reply_publish_failed = true;
                                    }
                                    Ok(confirm) => match confirm.await {
                                        Ok(confirmation) => {
                                            if let Err(e) = require_publisher_ack(confirmation) {
                                                tracing::error!(queue = %queue, "reply publish was not acknowledged: {e}");
                                                reply_publish_failed = true;
                                            }
                                        }
                                        Err(e) => {
                                            tracing::error!(queue = %queue, "failed to confirm reply publish: {e}");
                                            reply_publish_failed = true;
                                        }
                                    },
                                }
                            }

                            if reply_publish_failed {
                                // The reply never made it out — nack+requeue so the
                                // request is redelivered (bounded by the broker's
                                // redelivery) rather than acked and lost. An error
                                // reply that published successfully still acks.
                                tracing::warn!(queue = %queue, "reply publish failed, requeueing request");
                                if let Err(e) = acker
                                    .nack(BasicNackOptions {
                                        requeue: true,
                                        ..BasicNackOptions::default()
                                    })
                                    .await
                                {
                                    tracing::error!(queue = %queue, "failed to nack request: {e}");
                                }
                            } else {
                                // Ack the request after the reply is published (or
                                // when there was no reply-to to answer).
                                if let Err(e) = acker.ack(BasicAckOptions::default()).await {
                                    tracing::error!(queue = %queue, "failed to ack request: {e}");
                                }
                            }
                        });
                    }
                    Some(Err(e)) => {
                        tracing::warn!(queue = %queue_name, "responder consumer error, reconnecting: {e}");
                        break;
                    }
                    None => {
                        tracing::info!(queue = %queue_name, "responder consumer stream ended");
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
