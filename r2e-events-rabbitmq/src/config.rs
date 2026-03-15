/// Configuration for connecting to a RabbitMQ server via AMQP 0-9-1.
#[derive(Clone, Debug)]
pub struct RabbitMqConfig {
    /// AMQP URI (e.g., "amqp://guest:guest@localhost:5672/%2f").
    pub uri: String,
    /// Topic exchange name used for all event routing.
    pub exchange: String,
    /// Consumer group prefix for queue names. Queues are named `{consumer_group}.{topic}`.
    pub consumer_group: String,
    /// Channel-level prefetch count (QoS). Controls how many unacked messages
    /// the broker delivers to a consumer at once.
    pub prefetch_count: u16,
    /// Whether queues survive broker restarts.
    pub durable: bool,
    /// Whether published messages survive broker restarts (delivery_mode = 2).
    pub persistent: bool,
    /// Whether to auto-declare the exchange and queues on first use.
    pub auto_create: bool,
    /// Optional per-message TTL in milliseconds. `None` means no TTL.
    pub message_ttl_ms: Option<u32>,
    /// Optional dead-letter exchange name for rejected/expired messages.
    pub dead_letter_exchange: Option<String>,
    /// AMQP heartbeat interval in seconds. `0` disables heartbeats.
    pub heartbeat: u16,
    /// Optional connection name shown in the RabbitMQ management UI.
    pub connection_name: Option<String>,
}

impl Default for RabbitMqConfig {
    fn default() -> Self {
        Self {
            uri: "amqp://guest:guest@localhost:5672/%2f".into(),
            exchange: "r2e-events".into(),
            consumer_group: "r2e-app".into(),
            prefetch_count: 10,
            durable: true,
            persistent: true,
            auto_create: true,
            message_ttl_ms: None,
            dead_letter_exchange: None,
            heartbeat: 60,
            connection_name: None,
        }
    }
}

impl RabbitMqConfig {
    /// Create a new builder for `RabbitMqConfig`.
    pub fn builder() -> RabbitMqConfigBuilder {
        RabbitMqConfigBuilder::default()
    }
}

/// Builder for [`RabbitMqConfig`].
#[derive(Default)]
pub struct RabbitMqConfigBuilder {
    config: RabbitMqConfig,
}

impl RabbitMqConfigBuilder {
    pub fn uri(mut self, uri: impl Into<String>) -> Self {
        self.config.uri = uri.into();
        self
    }

    pub fn exchange(mut self, exchange: impl Into<String>) -> Self {
        self.config.exchange = exchange.into();
        self
    }

    pub fn consumer_group(mut self, group: impl Into<String>) -> Self {
        self.config.consumer_group = group.into();
        self
    }

    pub fn prefetch_count(mut self, count: u16) -> Self {
        self.config.prefetch_count = count;
        self
    }

    pub fn durable(mut self, durable: bool) -> Self {
        self.config.durable = durable;
        self
    }

    pub fn persistent(mut self, persistent: bool) -> Self {
        self.config.persistent = persistent;
        self
    }

    pub fn auto_create(mut self, auto_create: bool) -> Self {
        self.config.auto_create = auto_create;
        self
    }

    pub fn message_ttl_ms(mut self, ttl: u32) -> Self {
        self.config.message_ttl_ms = Some(ttl);
        self
    }

    pub fn dead_letter_exchange(mut self, exchange: impl Into<String>) -> Self {
        self.config.dead_letter_exchange = Some(exchange.into());
        self
    }

    pub fn heartbeat(mut self, seconds: u16) -> Self {
        self.config.heartbeat = seconds;
        self
    }

    pub fn connection_name(mut self, name: impl Into<String>) -> Self {
        self.config.connection_name = Some(name.into());
        self
    }

    pub fn build(self) -> RabbitMqConfig {
        self.config
    }
}
