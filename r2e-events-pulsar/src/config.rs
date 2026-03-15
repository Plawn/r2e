use pulsar::message::proto::command_subscribe::SubType;

/// Subscription type for Pulsar consumers.
///
/// Maps directly to Pulsar's `SubType` enum.
#[derive(Clone, Debug, Default)]
pub enum SubscriptionType {
    /// All messages are delivered to every consumer (round-robin).
    #[default]
    Shared,
    /// Only one consumer receives messages at a time.
    Exclusive,
    /// Standby consumers take over if the active one disconnects.
    Failover,
    /// Messages with the same key are delivered to the same consumer.
    KeyShared,
}

impl SubscriptionType {
    /// Convert to the Pulsar protocol `SubType`.
    pub fn to_sub_type(&self) -> SubType {
        match self {
            SubscriptionType::Shared => SubType::Shared,
            SubscriptionType::Exclusive => SubType::Exclusive,
            SubscriptionType::Failover => SubType::Failover,
            SubscriptionType::KeyShared => SubType::KeyShared,
        }
    }
}

/// Configuration for connecting to an Apache Pulsar cluster.
#[derive(Clone, Debug)]
pub struct PulsarConfig {
    /// Pulsar service URL (e.g., "pulsar://localhost:6650").
    pub service_url: String,
    /// Subscription name for this application instance.
    pub subscription: String,
    /// Subscription type (Shared, Exclusive, Failover, KeyShared).
    pub subscription_type: SubscriptionType,
    /// Topic name prefix (e.g., "persistent://public/default/").
    pub topic_prefix: String,
    /// Optional JWT authentication token.
    pub auth_token: Option<String>,
    /// Whether to verify TLS hostnames.
    pub tls_hostname_verification: bool,
    /// Number of messages to fetch per batch.
    pub batch_size: u32,
    /// Whether Pulsar should auto-create topics (Pulsar allows by default).
    pub auto_create: bool,
    /// Default number of partitions for topics (0 = non-partitioned).
    pub default_partitions: u32,
    /// Whether to automatically reconnect when the consumer disconnects (default: true).
    pub reconnect: bool,
    /// Maximum backoff between reconnection attempts (default: 60s).
    pub reconnect_max_backoff: std::time::Duration,
}

impl Default for PulsarConfig {
    fn default() -> Self {
        Self {
            service_url: "pulsar://localhost:6650".into(),
            subscription: "r2e-app".into(),
            subscription_type: SubscriptionType::default(),
            topic_prefix: "persistent://public/default/".into(),
            auth_token: None,
            tls_hostname_verification: false,
            batch_size: 100,
            auto_create: true,
            default_partitions: 0,
            reconnect: true,
            reconnect_max_backoff: std::time::Duration::from_secs(60),
        }
    }
}

impl PulsarConfig {
    /// Create a new builder for `PulsarConfig`.
    pub fn builder() -> PulsarConfigBuilder {
        PulsarConfigBuilder::default()
    }

    /// Build the full topic name from a short topic name.
    pub fn full_topic_name(&self, topic: &str) -> String {
        format!("{}{}", self.topic_prefix, topic)
    }
}

/// Builder for [`PulsarConfig`].
#[derive(Default)]
pub struct PulsarConfigBuilder {
    config: PulsarConfig,
}

impl PulsarConfigBuilder {
    pub fn service_url(mut self, url: impl Into<String>) -> Self {
        self.config.service_url = url.into();
        self
    }

    pub fn subscription(mut self, subscription: impl Into<String>) -> Self {
        self.config.subscription = subscription.into();
        self
    }

    pub fn subscription_type(mut self, sub_type: SubscriptionType) -> Self {
        self.config.subscription_type = sub_type;
        self
    }

    pub fn topic_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.config.topic_prefix = prefix.into();
        self
    }

    pub fn auth_token(mut self, token: impl Into<String>) -> Self {
        self.config.auth_token = Some(token.into());
        self
    }

    pub fn tls_hostname_verification(mut self, enabled: bool) -> Self {
        self.config.tls_hostname_verification = enabled;
        self
    }

    pub fn batch_size(mut self, size: u32) -> Self {
        self.config.batch_size = size;
        self
    }

    pub fn auto_create(mut self, auto_create: bool) -> Self {
        self.config.auto_create = auto_create;
        self
    }

    pub fn default_partitions(mut self, partitions: u32) -> Self {
        self.config.default_partitions = partitions;
        self
    }

    pub fn reconnect(mut self, enable: bool) -> Self {
        self.config.reconnect = enable;
        self
    }

    pub fn reconnect_max_backoff(mut self, duration: std::time::Duration) -> Self {
        self.config.reconnect_max_backoff = duration;
        self
    }

    pub fn build(self) -> PulsarConfig {
        self.config
    }
}
