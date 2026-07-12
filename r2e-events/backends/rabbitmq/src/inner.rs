use std::sync::{Arc, RwLock as StdRwLock};

use lapin::options::{
    BasicQosOptions, ExchangeDeclareOptions, QueueBindOptions, QueueDeclareOptions,
};
use lapin::types::{AMQPValue, FieldTable, LongString, ShortString};
use lapin::{Channel, Connection, ConnectionProperties, ExchangeKind};
use tokio::sync::Mutex;

use r2e_events::backend::BackendState;
use r2e_events::EventBusError;

use crate::config::RabbitMqConfig;
use crate::error::map_lapin_error;

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

    /// Close the retained connection (and thus all channels) during shutdown.
    pub(crate) async fn close(&self) {
        let guard = self.connection.lock().await;
        if let Err(e) = guard.close(200, "shutdown".into()).await {
            tracing::warn!("error closing RabbitMQ connection: {e}");
        }
    }
}
