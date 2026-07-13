use std::sync::{Arc, RwLock as StdRwLock};

use futures_util::StreamExt;
use lapin::options::{
    BasicConsumeOptions, BasicQosOptions, ExchangeDeclareOptions, QueueBindOptions,
    QueueDeclareOptions,
};
use lapin::types::{AMQPValue, FieldTable, LongString, ShortString};
use lapin::{Channel, Connection, ConnectionProperties, ExchangeKind};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use r2e_events::backend::{BackendState, PendingRequests, HEADER_REPLY_ERROR};
use r2e_events::EventBusError;

use crate::config::RabbitMqConfig;
use crate::error::map_lapin_error;

/// The RabbitMQ Direct Reply-To pseudo-queue. Consuming it (with `no_ack`) and
/// publishing a request tagged `reply_to = amq.rabbitmq.reply-to` on the SAME
/// channel gives a broker-managed, connection-scoped reply address with no
/// declared reply queue — the classic low-latency AMQP RPC transport.
pub(crate) const DIRECT_REPLY_TO: &str = "amq.rabbitmq.reply-to";

/// Shared inner state for `RabbitMqEventBus`, behind an `Arc`.
///
/// Owns the AMQP [`Connection`] (retained so channels can be recreated after a
/// broker blip) and a lazily-created publisher channel that is isolated from
/// every consumer channel. Each consumer creates and owns its own channel via
/// [`RabbitMqInner::new_consumer_channel`], so a failed publish only tears down
/// the publisher channel — never the consumers — and a dropped connection is
/// transparently re-established on the next channel request.
pub(crate) struct RabbitMqInner {
    pub config: RabbitMqConfig,
    /// The AMQP connection, guarded by a mutex so that at most one reconnect
    /// runs at a time even when several channels notice the drop concurrently.
    connection: Mutex<Connection>,
    /// Lazily-(re)created publisher channel, separate from consumer channels.
    /// Held in a std `RwLock` so the publish fast path only takes a read lock,
    /// checks `connected()` and clones the (Arc-backed) channel — no async
    /// mutex on the hot path. The guard is never held across an await.
    publisher: StdRwLock<Option<Channel>>,
    /// Serializes publisher-channel rebuilds so a broker blip triggers at most
    /// one reconnect even when many concurrent publishes miss the fast path.
    publisher_rebuild: Mutex<()>,
    /// Lazily-(re)created requester channel for request-reply. Direct Reply-To
    /// requires the reply consumer and the request publish to share one channel,
    /// so a single dedicated channel carries both. Stored the same way as the
    /// publisher channel (std `RwLock`, fast-path clone, never held across await).
    requester: StdRwLock<Option<Channel>>,
    /// Serializes requester-channel rebuilds (and reply-consumer restarts).
    requester_rebuild: Mutex<()>,
    /// Correlation map of in-flight requests → their waiting reply channels.
    pub(crate) pending: Arc<PendingRequests>,
    /// Cancelled on shutdown so in-flight requesters fail promptly with
    /// [`EventBusError::Shutdown`] instead of blocking until their timeout.
    pub(crate) request_cancel: CancellationToken,
    pub state: Arc<BackendState>,
}

impl RabbitMqInner {
    pub(crate) fn new(
        config: RabbitMqConfig,
        connection: Connection,
        state: Arc<BackendState>,
    ) -> Self {
        Self {
            config,
            connection: Mutex::new(connection),
            publisher: StdRwLock::new(None),
            publisher_rebuild: Mutex::new(()),
            requester: StdRwLock::new(None),
            requester_rebuild: Mutex::new(()),
            pending: Arc::new(PendingRequests::new()),
            request_cancel: CancellationToken::new(),
            state,
        }
    }

    /// Build the AMQP connection properties for this backend.
    fn connection_props(config: &RabbitMqConfig) -> ConnectionProperties {
        // lapin 4 uses the tokio runtime by default (default-runtime feature),
        // so no explicit executor/reactor wiring is required.
        ConnectionProperties::default().with_connection_name(
            config
                .connection_name
                .clone()
                .unwrap_or_else(|| "r2e-events-rabbitmq".into())
                .into(),
        )
    }

    /// Open a fresh AMQP connection to the broker.
    pub(crate) async fn connect(config: &RabbitMqConfig) -> Result<Connection, EventBusError> {
        Connection::connect(&config.uri, Self::connection_props(config))
            .await
            .map_err(map_lapin_error)
    }

    /// Create a fresh channel, transparently reconnecting the underlying
    /// connection if the broker link has dropped. Serialized through the
    /// connection mutex, so concurrent callers open at most one new connection.
    async fn create_channel(&self) -> Result<Channel, EventBusError> {
        let mut guard = self.connection.lock().await;
        if guard.status().connected() {
            if let Ok(channel) = guard.create_channel().await {
                return Ok(channel);
            }
            // Connection reported connected but channel creation failed — the
            // link is probably dead; fall through and reconnect.
        }

        let new_conn = Self::connect(&self.config).await?;
        let channel = new_conn.create_channel().await.map_err(map_lapin_error)?;
        *guard = new_conn;
        Ok(channel)
    }

    /// Declare the topic exchange on `channel` when `auto_create` is enabled.
    /// Idempotent on the broker, so it is safe to run for every fresh channel.
    async fn declare_exchange(&self, channel: &Channel) -> Result<(), EventBusError> {
        if self.config.auto_create {
            channel
                .exchange_declare(
                    self.config.exchange.as_str().into(),
                    ExchangeKind::Topic,
                    ExchangeDeclareOptions {
                        durable: self.config.durable,
                        ..ExchangeDeclareOptions::default()
                    },
                    FieldTable::default(),
                )
                .await
                .map_err(map_lapin_error)?;
        }
        Ok(())
    }

    /// Create a dedicated consumer channel: reconnect if needed, apply QoS
    /// (prefetch) and declare the exchange.
    pub(crate) async fn new_consumer_channel(&self) -> Result<Channel, EventBusError> {
        let channel = self.create_channel().await?;
        channel
            .basic_qos(self.config.prefetch_count, BasicQosOptions::default())
            .await
            .map_err(map_lapin_error)?;
        self.declare_exchange(&channel).await?;
        channel
            .confirm_select(lapin::options::ConfirmSelectOptions::default())
            .await
            .map_err(map_lapin_error)?;
        Ok(channel)
    }

    /// Return a live publisher channel, creating (or recreating) it when the
    /// stored one is missing or its connection has dropped. The publisher
    /// channel is separate from every consumer channel, so a failed publish
    /// never takes down a consumer.
    pub(crate) async fn publisher_channel(&self) -> Result<Channel, EventBusError> {
        // Fast path: clone out a live channel under a std read lock. The lapin
        // `Channel` clone is Arc-backed and cheap; the guard is dropped before
        // any await.
        if let Some(channel) = self.connected_publisher() {
            return Ok(channel);
        }

        // Slow path: rebuild. Serialize rebuilds through the async mutex so a
        // burst of concurrent missers opens at most one new channel, and
        // re-check the fast path after acquiring in case another task already
        // rebuilt while we waited.
        let _rebuild = self.publisher_rebuild.lock().await;
        if let Some(channel) = self.connected_publisher() {
            return Ok(channel);
        }

        let channel = self.create_channel().await?;
        self.declare_exchange(&channel).await?;
        channel
            .confirm_select(lapin::options::ConfirmSelectOptions::default())
            .await
            .map_err(map_lapin_error)?;
        // Store the fresh channel back through the write guard; the guard is
        // dropped immediately, never held across an await.
        *self.publisher.write().unwrap_or_else(|e| e.into_inner()) = Some(channel.clone());
        Ok(channel)
    }

    /// Fast-path read of the publisher channel: returns a clone iff a channel is
    /// stored and its connection is live. Holds the std read lock only for the
    /// clone (no await).
    fn connected_publisher(&self) -> Option<Channel> {
        let guard = self.publisher.read().unwrap_or_else(|e| e.into_inner());
        match guard.as_ref() {
            Some(channel) if channel.status().connected() => Some(channel.clone()),
            _ => None,
        }
    }

    /// Queue name for a resolved topic: `{consumer_group}.{topic_name}`.
    pub(crate) fn queue_name(&self, topic_name: &str) -> String {
        format!("{}.{}", self.config.consumer_group, topic_name)
    }

    /// Declare and bind the consumer queue for `topic_name` on `channel`.
    ///
    /// Runs on every (re)connect so the queue and its binding are re-established
    /// if the broker lost them across a restart. Idempotent on the broker.
    pub(crate) async fn ensure_queue(
        &self,
        channel: &Channel,
        topic_name: &str,
    ) -> Result<String, EventBusError> {
        let queue_name = self.queue_name(topic_name);

        if !self.config.auto_create {
            return Ok(queue_name);
        }

        let mut args = FieldTable::default();

        if let Some(ttl) = self.config.message_ttl_ms {
            args.insert(ShortString::from("x-message-ttl"), AMQPValue::LongUInt(ttl));
        }

        if let Some(ref dlx) = self.config.dead_letter_exchange {
            args.insert(
                ShortString::from("x-dead-letter-exchange"),
                AMQPValue::LongString(LongString::from(dlx.as_bytes())),
            );
        }

        channel
            .queue_declare(
                queue_name.as_str().into(),
                QueueDeclareOptions {
                    durable: self.config.durable,
                    ..QueueDeclareOptions::default()
                },
                args,
            )
            .await
            .map_err(map_lapin_error)?;

        channel
            .queue_bind(
                queue_name.as_str().into(),
                self.config.exchange.as_str().into(),
                topic_name.into(),
                QueueBindOptions::default(),
                FieldTable::default(),
            )
            .await
            .map_err(map_lapin_error)?;

        tracing::info!(
            queue = %queue_name,
            exchange = %self.config.exchange,
            routing_key = %topic_name,
            "declared and bound queue"
        );

        Ok(queue_name)
    }

    /// Declare and bind the shared request-reply queue for `request_topic`.
    ///
    /// Unlike [`ensure_queue`], the queue is named after the request topic
    /// itself — with **no** `consumer_group` prefix — so every instance of every
    /// consumer group consumes the one shared queue. The broker then load-balances
    /// requests across instances (competing consumers), delivering each request
    /// to exactly one responder. Durable and bound with routing key =
    /// `request_topic`, mirroring the normal event-queue declaration.
    pub(crate) async fn ensure_request_queue(
        &self,
        channel: &Channel,
        request_topic: &str,
    ) -> Result<String, EventBusError> {
        if !self.config.auto_create {
            return Ok(request_topic.to_string());
        }

        channel
            .queue_declare(
                request_topic.into(),
                QueueDeclareOptions {
                    durable: self.config.durable,
                    ..QueueDeclareOptions::default()
                },
                FieldTable::default(),
            )
            .await
            .map_err(map_lapin_error)?;

        channel
            .queue_bind(
                request_topic.into(),
                self.config.exchange.as_str().into(),
                request_topic.into(),
                QueueBindOptions::default(),
                FieldTable::default(),
            )
            .await
            .map_err(map_lapin_error)?;

        tracing::info!(
            queue = %request_topic,
            exchange = %self.config.exchange,
            "declared and bound shared request queue"
        );

        Ok(request_topic.to_string())
    }

    /// Return a live requester channel with its Direct Reply-To reply consumer
    /// running, creating (or recreating) it when the stored one is missing or
    /// its connection has dropped.
    ///
    /// The reply consumer runs on the same channel the caller then publishes on
    /// (a Direct Reply-To requirement) and routes every incoming reply into
    /// [`PendingRequests::complete`] by its `correlation_id`.
    pub(crate) async fn requester_channel(&self) -> Result<Channel, EventBusError> {
        // Fast path: a live channel is already set up.
        if let Some(channel) = self.connected_requester() {
            return Ok(channel);
        }

        // Slow path: rebuild under the async mutex, re-checking after acquiring
        // so a burst of concurrent missers opens at most one new channel +
        // reply consumer.
        let _rebuild = self.requester_rebuild.lock().await;
        if let Some(channel) = self.connected_requester() {
            return Ok(channel);
        }

        let channel = self.create_channel().await?;
        self.declare_exchange(&channel).await?;
        channel
            .confirm_select(lapin::options::ConfirmSelectOptions::default())
            .await
            .map_err(map_lapin_error)?;

        // Start the Direct Reply-To consumer on this channel (no_ack: replies on
        // the pseudo-queue are auto-acked by the broker).
        let consumer = channel
            .basic_consume(
                DIRECT_REPLY_TO.into(),
                format!("r2e-reply-{:016x}", r2e_events::backend::process_id())
                    .as_str()
                    .into(),
                BasicConsumeOptions {
                    no_ack: true,
                    ..BasicConsumeOptions::default()
                },
                FieldTable::default(),
            )
            .await
            .map_err(map_lapin_error)?;

        let pending = self.pending.clone();
        r2e_core::rt::spawn(async move {
            let mut consumer = consumer;
            while let Some(next) = consumer.next().await {
                match next {
                    Ok(delivery) => route_reply(&pending, &delivery),
                    // Stream error/end = channel dropped; exit so the next
                    // request rebuilds the channel + consumer.
                    Err(_) => break,
                }
            }
        });

        *self.requester.write().unwrap_or_else(|e| e.into_inner()) = Some(channel.clone());
        Ok(channel)
    }

    /// Fast-path read of the requester channel: a clone iff a channel is stored
    /// and its connection is live.
    fn connected_requester(&self) -> Option<Channel> {
        let guard = self.requester.read().unwrap_or_else(|e| e.into_inner());
        match guard.as_ref() {
            Some(channel) if channel.status().connected() => Some(channel.clone()),
            _ => None,
        }
    }

    /// Close the retained connection (and thus all channels) during shutdown.
    pub(crate) async fn close(&self) {
        let guard = self.connection.lock().await;
        if let Err(e) = guard.close(200, "shutdown".into()).await {
            tracing::warn!("error closing RabbitMQ connection: {e}");
        }
    }
}

/// Route a single Direct Reply-To delivery into the correlation map.
///
/// The `correlation_id` AMQP property carries the requester's pending id; the
/// [`HEADER_REPLY_ERROR`] header, when present, marks a responder failure that
/// surfaces to the requester as [`EventBusError::Remote`]. Unknown ids (a reply
/// for a request that already timed out and evicted its entry) are dropped.
fn route_reply(pending: &PendingRequests, delivery: &lapin::message::Delivery) {
    let Some(id) = delivery
        .properties
        .correlation_id()
        .as_ref()
        .and_then(|s| s.as_str().parse::<u128>().ok())
    else {
        return;
    };

    let result = match reply_error_from_delivery(delivery) {
        Some(msg) => Err(EventBusError::Remote(msg)),
        None => Ok(delivery.data.clone()),
    };
    pending.complete(id, result);
}

/// Read the [`HEADER_REPLY_ERROR`] header off a reply delivery, if present.
fn reply_error_from_delivery(delivery: &lapin::message::Delivery) -> Option<String> {
    let headers = delivery.properties.headers().as_ref()?;
    for (key, value) in headers.inner() {
        if key.as_str() == HEADER_REPLY_ERROR {
            return Some(match value {
                AMQPValue::LongString(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
                AMQPValue::ShortString(s) => s.to_string(),
                other => format!("{other:?}"),
            });
        }
    }
    None
}
