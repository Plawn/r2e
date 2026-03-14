use std::time::Duration;

/// Transport protocol for connecting to the Iggy server.
#[derive(Clone, Debug, Default)]
pub enum Transport {
    /// TCP transport (default, port 8090).
    #[default]
    Tcp,
    /// QUIC transport.
    Quic,
    /// HTTP transport.
    Http,
}

/// Configuration for connecting to an Apache Iggy server.
#[derive(Clone, Debug)]
pub struct IggyConfig {
    /// Server address (e.g., "127.0.0.1:8090").
    pub address: String,
    /// Transport protocol.
    pub transport: Transport,
    /// Iggy stream name to use for all events.
    pub stream_name: String,
    /// Consumer group name for this application instance.
    pub consumer_group: String,
    /// Username for authentication.
    pub username: Option<String>,
    /// Password for authentication.
    pub password: Option<String>,
    /// How often to poll for new messages.
    pub poll_interval: Duration,
    /// Number of messages to fetch per poll.
    pub poll_batch_size: u32,
    /// Whether to auto-create streams, topics, and consumer groups.
    pub auto_create: bool,
    /// Default number of partitions for auto-created topics.
    pub default_partitions: u32,
}

impl Default for IggyConfig {
    fn default() -> Self {
        Self {
            address: "127.0.0.1:8090".into(),
            transport: Transport::default(),
            stream_name: "r2e-events".into(),
            consumer_group: "r2e-app".into(),
            username: None,
            password: None,
            poll_interval: Duration::from_millis(100),
            poll_batch_size: 100,
            auto_create: true,
            default_partitions: 1,
        }
    }
}

impl IggyConfig {
    /// Create a new builder for `IggyConfig`.
    pub fn builder() -> IggyConfigBuilder {
        IggyConfigBuilder::default()
    }
}

/// Builder for [`IggyConfig`].
#[derive(Default)]
pub struct IggyConfigBuilder {
    config: IggyConfig,
}

impl IggyConfigBuilder {
    pub fn address(mut self, address: impl Into<String>) -> Self {
        self.config.address = address.into();
        self
    }

    pub fn transport(mut self, transport: Transport) -> Self {
        self.config.transport = transport;
        self
    }

    pub fn stream_name(mut self, name: impl Into<String>) -> Self {
        self.config.stream_name = name.into();
        self
    }

    pub fn consumer_group(mut self, group: impl Into<String>) -> Self {
        self.config.consumer_group = group.into();
        self
    }

    pub fn username(mut self, username: impl Into<String>) -> Self {
        self.config.username = Some(username.into());
        self
    }

    pub fn password(mut self, password: impl Into<String>) -> Self {
        self.config.password = Some(password.into());
        self
    }

    pub fn poll_interval(mut self, interval: Duration) -> Self {
        self.config.poll_interval = interval;
        self
    }

    pub fn poll_batch_size(mut self, size: u32) -> Self {
        self.config.poll_batch_size = size;
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

    pub fn build(self) -> IggyConfig {
        self.config
    }
}
