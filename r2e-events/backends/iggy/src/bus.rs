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
    await_reply, decode_metadata, decode_reply_headers, encode_metadata, encode_reply_headers,
    reconnect_loop, request_topic, responder_group, spawn_completion_forwarder, DispatchOutcome,
    Handler, ReplyHeaders, WatermarkTracker, COMPLETION_CHANNEL_CAPACITY, COMPLETION_DRAIN_TIMEOUT,
    HEADER_PARTITION_KEY, HEADER_TIMESTAMP,
};
use r2e_events::{
    EmitReceipt, EventBus, EventBusError, EventEnvelope, EventMetadata, HandlerResult,
    RequestOptions, ResponderHandle, SubscriptionHandle,
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
/// - `emit` is fan-out publish/subscribe; use `request`/`respond` for
///   point-to-point request-reply.
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
    fn resolve_topic<E: 'static>(&self) -> Arc<str> {
        self.inner.state.resolve_topic::<E>()
    }

    /// Ensure a topic exists in Iggy (idempotent, cached) with the configured
    /// default partition count.
    async fn ensure_topic(&self, topic_name: &str) -> Result<(), EventBusError> {
        self.ensure_topic_with_partitions(topic_name, self.inner.config.default_partitions)
            .await
    }

    /// Ensure a topic exists in Iggy (idempotent, cached) with an explicit
    /// partition count. Used for the per-process reply topic, which is
    /// single-partition and consumed by a standalone consumer.
    async fn ensure_topic_with_partitions(
        &self,
        topic_name: &str,
        partitions: u32,
    ) -> Result<(), EventBusError> {
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
                partitions,
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
        headers_from_pairs(encode_metadata(metadata))
    }

    /// Resolve (or cache) the `Identifier` for a topic name. The identifier is
    /// cached per topic string so `Identifier::named()` is not re-parsed on every
    /// publish.
    fn resolve_topic_id(&self, topic_name: &str) -> Result<Identifier, EventBusError> {
        {
            let cache = self
                .inner
                .topic_ids
                .read()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(id) = cache.get(topic_name) {
                return Ok(id.clone());
            }
        }
        let id = Identifier::named(topic_name).map_err(map_iggy_error)?;
        self.inner
            .topic_ids
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(Arc::from(topic_name), id.clone());
        Ok(id)
    }

    /// Send a single message to a topic with pre-built headers and an optional
    /// partition key. Ensures the topic exists first (idempotent).
    async fn send_message(
        &self,
        topic_name: &str,
        payload: Vec<u8>,
        headers: BTreeMap<HeaderKey, HeaderValue>,
        partition_key: Option<&str>,
    ) -> Result<(), EventBusError> {
        self.ensure_topic(topic_name).await?;

        let stream_id = &self.inner.stream_id;
        let topic_id = self.resolve_topic_id(topic_name)?;

        let partitioning = match partition_key {
            Some(key) => Partitioning::messages_key_str(key)
                .map_err(|e| EventBusError::Serialization(e.to_string()))?,
            None => Partitioning::balanced(),
        };

        let msg = IggyMessage::builder()
            .payload(bytes::Bytes::from(payload))
            .user_headers(headers)
            .build()
            .map_err(|e| EventBusError::Serialization(e.to_string()))?;

        self.inner
            .client
            .send_messages(stream_id, &topic_id, &partitioning, &mut [msg])
            .await
            .map_err(map_iggy_error)?;

        Ok(())
    }

    /// Publish a serialized event to Iggy.
    pub(crate) async fn publish(
        &self,
        topic_name: &str,
        payload: Vec<u8>,
        metadata: &EventMetadata,
    ) -> Result<(), EventBusError> {
        let headers = Self::build_headers(metadata)?;
        self.send_message(
            topic_name,
            payload,
            headers,
            metadata.partition_key.as_deref(),
        )
        .await
    }

    /// Ensure the per-process reply poller is running.
    ///
    /// Called on the first `request`: creates the instance-private reply topic
    /// (single-partition) and starts exactly one reply poller for the process.
    /// The poller routes incoming replies to [`PendingRequests`] by correlation
    /// id. Concurrent first-requests double-check under the lock so only one
    /// poller is ever spawned.
    async fn ensure_reply_poller_started(&self) -> Result<(), EventBusError> {
        {
            let guard = self
                .inner
                .rr_cancels
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if guard.reply_poller.is_some() {
                return Ok(());
            }
        }

        let reply = self.inner.reply_topic.clone();
        // Reply topic is instance-private and correlation-routed in-process, so
        // a single partition is sufficient (consumed by a standalone consumer).
        self.ensure_topic_with_partitions(&reply, 1).await?;

        let cancel = {
            let mut guard = self
                .inner
                .rr_cancels
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if guard.reply_poller.is_some() {
                return Ok(());
            }
            let cancel = CancellationToken::new();
            guard.reply_poller = Some(cancel.clone());
            cancel
        };

        let bus = self.clone();
        r2e_core::rt::spawn(async move {
            run_reply_poller(bus, reply, cancel).await;
        });

        Ok(())
    }
}

/// Partition id of the single-partition per-process reply topic (Iggy
/// partitions are 1-based).
const REPLY_PARTITION: u32 = 1;

/// Build an Iggy user-header map from encoded key/value pairs, skipping the
/// partition-key pseudo-header (that value drives Iggy partitioning, not
/// message headers).
#[doc(hidden)]
pub fn headers_from_pairs(
    pairs: impl IntoIterator<Item = (impl AsRef<str>, impl Into<String>)>,
) -> Result<BTreeMap<HeaderKey, HeaderValue>, EventBusError> {
    let mut headers = BTreeMap::new();
    for (k, v) in pairs {
        let k = k.as_ref();
        if k == HEADER_PARTITION_KEY {
            continue;
        }
        let v = v.into();
        headers.insert(
            HeaderKey::try_from(k)
                .map_err(|e: IggyError| EventBusError::Serialization(e.to_string()))?,
            HeaderValue::try_from(v.as_str())
                .map_err(|e: IggyError| EventBusError::Serialization(e.to_string()))?,
        );
    }
    Ok(headers)
}

/// Decode request-reply control headers from an Iggy message, or `None` when
/// the message carries no correlation id (not part of a request-reply exchange).
#[doc(hidden)]
pub fn reply_headers_from_message(message: &IggyMessage) -> Option<ReplyHeaders> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    if let Ok(Some(headers)) = message.user_headers_map() {
        for (key, value) in &headers {
            let (Ok(k), Ok(v)) = (key.as_str(), value.as_str()) else {
                continue;
            };
            pairs.push((k.to_string(), v.to_string()));
        }
    }
    decode_reply_headers(pairs.into_iter())
}

impl EventBus for IggyEventBus {
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
                let envelope = EventEnvelope { event, metadata: std::sync::Arc::new(metadata) };
                Box::pin(handler(envelope))
            });

            let (id, is_first) = inner.state.register_handler::<E>(h).await;

            // If this is the first subscriber for this type, set up the poller
            if is_first {
                if let Err(error) = bus.ensure_topic(&topic_name).await {
                    inner.state.unregister_handler(type_id, id).await;
                    return Err(error);
                }

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
                let envelope = EventEnvelope { event, metadata: std::sync::Arc::new(metadata) };
                Box::pin(handler(envelope))
            });

            let (id, is_first) = inner
                .state
                .register_handler_with_deserializer::<E>(h, deserializer)
                .await;

            if is_first {
                if let Err(error) = bus.ensure_topic(&topic_name).await {
                    inner.state.unregister_handler(type_id, id).await;
                    return Err(error);
                }

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

            let headers = Self::build_headers(&metadata)?;
            bus.ensure_topic(&topic_name).await?;

            let stream_id = bus.inner.stream_id.clone();
            let topic_id = bus.resolve_topic_id(&topic_name)?;
            let partitioning = match metadata.partition_key.as_deref() {
                Some(key) => Partitioning::messages_key_str(key)
                    .map_err(|e| EventBusError::Serialization(e.to_string()))?,
                None => Partitioning::balanced(),
            };
            let msg = IggyMessage::builder()
                .payload(bytes::Bytes::from(payload))
                .user_headers(headers)
                .build()
                .map_err(|e| EventBusError::Serialization(e.to_string()))?;

            let client = bus.inner.client.clone();
            let (tx, rx) = tokio::sync::oneshot::channel();
            r2e_core::rt::spawn(async move {
                let result = client
                    .send_messages(&stream_id, &topic_id, &partitioning, &mut [msg])
                    .await;
                let _ = tx.send(result.map_err(map_iggy_error));
            });

            Ok(EmitReceipt::new(async move {
                rx.await
                    .map_err(|_| EventBusError::Other("iggy send task dropped".to_string()))?
            }))
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

            let headers = Self::build_headers(&metadata)?;
            bus.ensure_topic(&topic_name).await?;

            let stream_id = bus.inner.stream_id.clone();
            let topic_id = bus.resolve_topic_id(&topic_name)?;
            let partitioning = match metadata.partition_key.as_deref() {
                Some(key) => Partitioning::messages_key_str(key)
                    .map_err(|e| EventBusError::Serialization(e.to_string()))?,
                None => Partitioning::balanced(),
            };
            let msg = IggyMessage::builder()
                .payload(bytes::Bytes::from(payload))
                .user_headers(headers)
                .build()
                .map_err(|e| EventBusError::Serialization(e.to_string()))?;

            let client = bus.inner.client.clone();
            let (tx, rx) = tokio::sync::oneshot::channel();
            r2e_core::rt::spawn(async move {
                let result = client
                    .send_messages(&stream_id, &topic_id, &partitioning, &mut [msg])
                    .await;
                let _ = tx.send(result.map_err(map_iggy_error));
            });

            Ok(EmitReceipt::new(async move {
                rx.await
                    .map_err(|_| EventBusError::Other("iggy send task dropped".to_string()))?
            }))
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

            // Serialize the request up front (so we fail before touching the
            // broker on a bad payload).
            let payload = serde_json::to_vec(&req)
                .map_err(|e| EventBusError::Serialization(e.to_string()))?;

            let base_topic = bus.resolve_topic::<Req>();
            let req_topic = request_topic(&base_topic);
            let reply_to = bus.inner.reply_topic.clone();

            // Ensure the per-process reply poller is live before we publish so a
            // fast reply is never missed.
            bus.ensure_reply_poller_started().await?;

            // Register the pending entry BEFORE publishing so an in-flight reply
            // has a slot to complete. The guard evicts the entry on drop
            // (timeout / shutdown / early return).
            let (request_id, _guard, rx) = bus.inner.pending.register();

            // Build request headers: metadata + reply-to control headers. The
            // request metadata's own correlation_id (a user string) is left
            // untouched; the u128 request-reply id travels via the reply headers
            // in their own dedicated header slot.
            let metadata = options.metadata.unwrap_or_default();
            let pairs = encode_metadata(&metadata).chain(encode_reply_headers(
                request_id,
                Some(&reply_to),
                None,
            ));
            let headers = headers_from_pairs(pairs)?;

            bus.send_message(
                &req_topic,
                payload,
                headers,
                metadata.partition_key.as_deref(),
            )
            .await?;

            // Await the reply, the request timeout, or the sticky shutdown
            // token. A missing responder is
            // not a silent drop: the responder poller always publishes an error
            // reply, so it surfaces as `EventBusError::Remote` rather than a
            // full-timeout wait.
            await_reply::<Resp>(rx, options.timeout, bus.inner.request_cancel.cancelled()).await
        }
    }

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

            // Register the single responder for this request type (errors if one
            // is already registered — at most one responder per type per process).
            bus.inner
                .state
                .register_responder::<Req, Resp, E, F, Fut>(handler)
                .await?;

            let type_id = TypeId::of::<Req>();
            let base_topic = bus.resolve_topic::<Req>();
            let req_topic = request_topic(&base_topic);
            if let Err(error) = bus.ensure_topic(&req_topic).await {
                bus.inner.state.unregister_responder(type_id).await;
                return Err(error);
            }

            let cancel = CancellationToken::new();
            {
                let mut guard = bus
                    .inner
                    .rr_cancels
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                guard.responder_pollers.push(cancel.clone());
            }

            let poller_bus = bus.clone();
            let poller_topic = req_topic.clone();
            let poller_cancel = cancel.clone();
            r2e_core::rt::spawn(async move {
                run_responder_poller(poller_bus, type_id, poller_topic, poller_cancel).await;
            });

            let inner = bus.inner.clone();
            let type_name = std::any::type_name::<Req>();
            Ok(ResponderHandle::new(type_name, move || {
                // Stop the responder poller and drop the responder registration.
                cancel.cancel();
                let inner = inner.clone();
                r2e_core::rt::spawn(async move {
                    inner.state.unregister_responder(type_id).await;
                });
            }))
        }
    }

    fn clear(&self) -> impl Future<Output = ()> + Send {
        let inner = self.inner.clone();
        async move {
            inner.state.cancel_all_pollers();
            // Stop the request-reply pollers and drop any responder registrations.
            {
                let mut rr = inner.rr_cancels.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(c) = rr.reply_poller.take() {
                    c.cancel();
                }
                for c in rr.responder_pollers.drain(..) {
                    c.cancel();
                }
            }
            inner.state.responders.write().await.clear();
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

            // Cancel the request-reply pollers and fail any pending requests:
            // waking the shutdown notifier makes requesters awaiting a reply
            // return `Shutdown` instead of waiting out their timeout.
            {
                let mut rr = inner.rr_cancels.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(c) = rr.reply_poller.take() {
                    c.cancel();
                }
                for c in rr.responder_pollers.drain(..) {
                    c.cancel();
                }
            }
            inner.request_cancel.cancel();

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
    topic_name: Arc<str>,
    cancel: CancellationToken,
) {
    let max_backoff = inner.config.reconnect_max_backoff;
    let reconnect = inner.config.reconnect;
    let label = format!("Iggy poller [{topic_name}]");
    reconnect_loop(reconnect, max_backoff, &cancel, &label, || {
        run_poller_inner(&inner, type_id, &topic_name, &cancel)
    })
    .await;
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
                while let Ok(((partition_id, offset), outcome)) = rx.try_recv() {
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
            apply_completion(
                &mut tracker,
                consumer,
                topic_name,
                partition_id,
                offset,
                outcome,
            )
            .await;
        }
    };
    if r2e_core::rt::timeout(COMPLETION_DRAIN_TIMEOUT, drain)
        .await
        .is_err()
    {
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
    pairs.push((
        HEADER_TIMESTAMP.to_string(),
        message.header.timestamp.to_string(),
    ));

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

// ── Request-reply pollers ──────────────────────────────────────────────

/// Per-process reply poller with automatic reconnection.
///
/// Consumes the instance-private reply topic with a standalone (non-group)
/// consumer so only this process reads its own replies, and routes each reply
/// to the waiting requester by correlation id.
async fn run_reply_poller(bus: IggyEventBus, reply_topic_name: String, cancel: CancellationToken) {
    let max_backoff = bus.inner.config.reconnect_max_backoff;
    let reconnect = bus.inner.config.reconnect;
    let label = format!("Iggy reply poller [{reply_topic_name}]");
    reconnect_loop(reconnect, max_backoff, &cancel, &label, || {
        run_reply_poller_inner(&bus, &reply_topic_name, &cancel)
    })
    .await;
}

async fn run_reply_poller_inner(
    bus: &IggyEventBus,
    reply_topic_name: &str,
    cancel: &CancellationToken,
) {
    let inner = &bus.inner;
    // Process-unique standalone consumer: only this instance consumes its own
    // reply topic. Default (interval) auto-commit is fine — a lost reply just
    // becomes a `RequestTimeout`, and offsets should advance so old replies are
    // not reprocessed after a reconnect.
    let consumer_name = format!("r2e-reply-{:016x}", inner.instance_id);
    let consumer_result = inner.client.consumer(
        &consumer_name,
        &inner.config.stream_name,
        reply_topic_name,
        REPLY_PARTITION,
    );

    let mut consumer = match consumer_result {
        Ok(builder) => builder
            .polling_strategy(PollingStrategy::next())
            .poll_interval(IggyDuration::from(inner.config.poll_interval))
            .batch_length(inner.config.poll_batch_size)
            .build(),
        Err(e) => {
            tracing::error!(topic = %reply_topic_name, "failed to create Iggy reply consumer: {e}");
            return;
        }
    };

    if let Err(e) = consumer.init().await {
        tracing::error!(topic = %reply_topic_name, "failed to init Iggy reply consumer: {e}");
        return;
    }

    tracing::info!(topic = %reply_topic_name, "reply poller started");

    let batch_capacity = (inner.config.poll_batch_size.max(1)) as usize;
    let mut batches = futures_util::StreamExt::ready_chunks(consumer, batch_capacity);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::info!(topic = %reply_topic_name, "reply poller cancelled");
                break;
            }
            batch = futures_util::StreamExt::next(&mut batches) => {
                let batch = match batch {
                    Some(batch) => batch,
                    None => {
                        tracing::info!(topic = %reply_topic_name, "reply consumer stream ended");
                        break;
                    }
                };

                for item in &batch {
                    match item {
                        Ok(received) => {
                            let Some(headers) = reply_headers_from_message(&received.message) else {
                                tracing::debug!(topic = %reply_topic_name, "reply without request id, ignored");
                                continue;
                            };
                            // Single-sources the Remote-vs-Ok decision and
                            // routes the reply to the waiting requester by id.
                            inner.pending.complete_reply(&headers, received.message.payload.to_vec());
                        }
                        Err(e) => {
                            tracing::warn!(topic = %reply_topic_name, "reply poll error: {e}");
                        }
                    }
                }
            }
        }
    }
}

/// Responder poller with automatic reconnection.
///
/// Consumes the shared request topic via its deterministic responder group
/// (broker-side load balancing across all instances), invokes the registered responder, sends
/// the reply, then commits the request offset — reply-then-commit for
/// at-least-once delivery.
async fn run_responder_poller(
    bus: IggyEventBus,
    type_id: TypeId,
    req_topic: String,
    cancel: CancellationToken,
) {
    let max_backoff = bus.inner.config.reconnect_max_backoff;
    let reconnect = bus.inner.config.reconnect;
    let label = format!("Iggy responder poller [{req_topic}]");
    reconnect_loop(reconnect, max_backoff, &cancel, &label, || {
        run_responder_poller_inner(&bus, type_id, &req_topic, &cancel)
    })
    .await;
}

async fn run_responder_poller_inner(
    bus: &IggyEventBus,
    type_id: TypeId,
    req_topic: &str,
    cancel: &CancellationToken,
) {
    let inner = &bus.inner;
    let group = responder_group(req_topic);
    let consumer_result = inner
        .client
        .consumer_group(&group, &inner.config.stream_name, req_topic);

    let mut consumer = match consumer_result {
        Ok(builder) => builder
            .auto_commit(AutoCommit::Disabled)
            .create_consumer_group_if_not_exists()
            .auto_join_consumer_group()
            .polling_strategy(PollingStrategy::next())
            .poll_interval(IggyDuration::from(inner.config.poll_interval))
            .batch_length(inner.config.poll_batch_size)
            .build(),
        Err(e) => {
            tracing::error!(topic = %req_topic, "failed to create Iggy responder consumer: {e}");
            return;
        }
    };

    if let Err(e) = consumer.init().await {
        tracing::error!(topic = %req_topic, "failed to init Iggy responder consumer: {e}");
        return;
    }

    tracing::info!(topic = %req_topic, "responder poller started");

    // Pipelined: requests are dispatched as they arrive; completions flow back
    // on a bounded channel. A watermark tracker advances the commit offset over
    // the contiguous prefix of completed requests, same as the regular poller.
    let batch_capacity = (inner.config.poll_batch_size.max(1)) as usize;
    let mut batches = futures_util::StreamExt::ready_chunks(consumer, batch_capacity);

    let mut tracker: WatermarkTracker<u32, u64> = WatermarkTracker::new();
    let (tx, mut rx) =
        tokio::sync::mpsc::channel::<((u32, u64), bool)>(COMPLETION_CHANNEL_CAPACITY);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::info!(topic = %req_topic, "responder poller cancelled");
                break;
            }
            Some(((partition_id, offset), ok)) = rx.recv() => {
                apply_responder_completion(
                    &mut tracker,
                    batches.get_ref(),
                    req_topic,
                    partition_id,
                    offset,
                    ok,
                )
                .await;
                while let Ok(((partition_id, offset), ok)) = rx.try_recv() {
                    apply_responder_completion(
                        &mut tracker,
                        batches.get_ref(),
                        req_topic,
                        partition_id,
                        offset,
                        ok,
                    )
                    .await;
                }
            }
            batch = futures_util::StreamExt::next(&mut batches) => {
                let batch = match batch {
                    Some(batch) => batch,
                    None => {
                        tracing::info!(topic = %req_topic, "responder consumer stream ended");
                        break;
                    }
                };

                for item in &batch {
                    match item {
                        Ok(received) => {
                            let partition_id = received.partition_id;
                            let offset = received.message.header.offset;
                            tracker.on_receive(partition_id, offset);

                            let bus = bus.clone();
                            let tx = tx.clone();
                            let message_payload = received.message.payload.to_vec();
                            let message_headers = extract_metadata_from_message(&received.message);
                            let reply_hdrs = reply_headers_from_message(&received.message);
                            r2e_core::rt::spawn(async move {
                                let ok = handle_request(
                                    &bus, type_id, &message_payload, message_headers, reply_hdrs,
                                )
                                .await;
                                let _ = tx.send(((partition_id, offset), ok)).await;
                            });
                        }
                        Err(e) => {
                            tracing::warn!(topic = %req_topic, "responder poll error: {e}");
                        }
                    }
                }
            }
        }
    }

    // Drain pending completions best-effort before dropping the consumer.
    drop(tx);
    let consumer = batches.get_ref();
    let drain = async {
        while let Some(((partition_id, offset), ok)) = rx.recv().await {
            apply_responder_completion(&mut tracker, consumer, req_topic, partition_id, offset, ok)
                .await;
        }
    };
    if r2e_core::rt::timeout(COMPLETION_DRAIN_TIMEOUT, drain)
        .await
        .is_err()
    {
        tracing::warn!(
            topic = %req_topic,
            "timed out draining responder completions; \
             undrained messages will be redelivered on restart"
        );
    }
}

/// Apply one responder completion to the watermark tracker.
async fn apply_responder_completion(
    tracker: &mut WatermarkTracker<u32, u64>,
    consumer: &IggyConsumer,
    topic_name: &str,
    partition_id: u32,
    offset: u64,
    ok: bool,
) {
    if ok {
        if let Some(commit) = tracker.on_ack(partition_id, offset) {
            if let Err(e) = consumer.store_offset(commit, Some(partition_id)).await {
                tracing::warn!(
                    topic = %topic_name,
                    partition_id,
                    offset = commit,
                    "failed to store responder offset: {e}"
                );
            }
        }
    } else {
        tracker.on_nack(partition_id, offset);
        tracing::warn!(
            topic = %topic_name,
            partition_id,
            offset,
            "reply publish failed — partition pinned, request redelivered on restart"
        );
    }
}

/// Handle one request: invoke the responder and publish the reply. Works with
/// pre-extracted owned values so it can run in a spawned task.
async fn handle_request(
    bus: &IggyEventBus,
    type_id: TypeId,
    payload: &[u8],
    metadata: EventMetadata,
    reply_hdrs: Option<ReplyHeaders>,
) -> bool {
    let Some(headers) = reply_hdrs else {
        tracing::debug!("request without reply headers, skipping");
        return true;
    };
    let Some(reply_to) = headers.reply_to else {
        tracing::debug!("request without reply-to, skipping");
        return true;
    };

    let (reply_payload, reply_error) = bus
        .inner
        .state
        .build_reply(type_id, payload, metadata)
        .await;

    let reply_headers = match headers_from_pairs(encode_reply_headers(
        headers.request_id,
        None,
        reply_error.as_deref(),
    )) {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!("failed to build reply headers: {e}");
            return true;
        }
    };

    match bus
        .send_message(&reply_to, reply_payload, reply_headers, None)
        .await
    {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(topic = %reply_to, "failed to send reply: {e}");
            false
        }
    }
}
