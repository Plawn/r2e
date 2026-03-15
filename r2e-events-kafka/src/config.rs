use std::collections::HashMap;

/// Security protocol for connecting to Kafka.
#[derive(Clone, Debug, Default)]
pub enum SecurityProtocol {
    /// No encryption or authentication.
    #[default]
    Plaintext,
    /// TLS encryption without SASL authentication.
    Ssl,
    /// SASL authentication without encryption.
    SaslPlaintext,
    /// SASL authentication with TLS encryption.
    SaslSsl,
}

impl SecurityProtocol {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Plaintext => "plaintext",
            Self::Ssl => "ssl",
            Self::SaslPlaintext => "sasl_plaintext",
            Self::SaslSsl => "sasl_ssl",
        }
    }
}

/// Compression algorithm for produced messages.
#[derive(Clone, Debug, Default)]
pub enum Compression {
    /// No compression (default).
    #[default]
    None,
    Gzip,
    Snappy,
    Lz4,
    Zstd,
}

impl Compression {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Gzip => "gzip",
            Self::Snappy => "snappy",
            Self::Lz4 => "lz4",
            Self::Zstd => "zstd",
        }
    }
}

/// Acknowledgment level for produced messages.
#[derive(Clone, Debug, Default)]
pub enum Acks {
    /// No acknowledgment.
    Zero,
    /// Leader acknowledgment only.
    One,
    /// Full ISR acknowledgment (default).
    #[default]
    All,
}

impl Acks {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Zero => "0",
            Self::One => "1",
            Self::All => "all",
        }
    }
}

/// Configuration for connecting to an Apache Kafka cluster.
#[derive(Clone, Debug)]
pub struct KafkaConfig {
    /// Bootstrap servers (e.g., "localhost:9092").
    pub bootstrap_servers: String,
    /// Consumer group ID.
    pub group_id: String,
    /// Security protocol.
    pub security_protocol: SecurityProtocol,
    /// SASL mechanism (e.g., "PLAIN", "SCRAM-SHA-256", "SCRAM-SHA-512").
    pub sasl_mechanism: Option<String>,
    /// SASL username.
    pub sasl_username: Option<String>,
    /// SASL password.
    pub sasl_password: Option<String>,
    /// Compression algorithm for produced messages.
    pub compression: Compression,
    /// Acknowledgment level.
    pub acks: Acks,
    /// Whether to auto-create topics using AdminClient.
    pub auto_create: bool,
    /// Default number of partitions for auto-created topics.
    pub default_partitions: i32,
    /// Default replication factor for auto-created topics.
    pub default_replication_factor: i32,
    /// Session timeout in milliseconds.
    pub session_timeout_ms: u32,
    /// Whether to enable auto-commit of consumer offsets.
    pub enable_auto_commit: bool,
    /// Extra librdkafka configuration overrides.
    pub overrides: HashMap<String, String>,
}

impl Default for KafkaConfig {
    fn default() -> Self {
        Self {
            bootstrap_servers: "localhost:9092".into(),
            group_id: "r2e-app".into(),
            security_protocol: SecurityProtocol::default(),
            sasl_mechanism: None,
            sasl_username: None,
            sasl_password: None,
            compression: Compression::default(),
            acks: Acks::default(),
            auto_create: true,
            default_partitions: 1,
            default_replication_factor: 1,
            session_timeout_ms: 30000,
            enable_auto_commit: true,
            overrides: HashMap::new(),
        }
    }
}

impl KafkaConfig {
    /// Create a new builder for `KafkaConfig`.
    pub fn builder() -> KafkaConfigBuilder {
        KafkaConfigBuilder::default()
    }

    /// Build an rdkafka `ClientConfig` for the producer.
    pub(crate) fn to_producer_client_config(&self) -> rdkafka::ClientConfig {
        let mut config = rdkafka::ClientConfig::new();
        config
            .set("bootstrap.servers", &self.bootstrap_servers)
            .set("security.protocol", self.security_protocol.as_str())
            .set("compression.type", self.compression.as_str())
            .set("acks", self.acks.as_str());

        if let Some(ref mechanism) = self.sasl_mechanism {
            config.set("sasl.mechanism", mechanism);
        }
        if let Some(ref username) = self.sasl_username {
            config.set("sasl.username", username);
        }
        if let Some(ref password) = self.sasl_password {
            config.set("sasl.password", password);
        }

        for (k, v) in &self.overrides {
            config.set(k, v);
        }

        config
    }

    /// Build an rdkafka `ClientConfig` for the consumer.
    pub(crate) fn to_consumer_client_config(&self) -> rdkafka::ClientConfig {
        let mut config = rdkafka::ClientConfig::new();
        config
            .set("bootstrap.servers", &self.bootstrap_servers)
            .set("group.id", &self.group_id)
            .set("security.protocol", self.security_protocol.as_str())
            .set(
                "session.timeout.ms",
                self.session_timeout_ms.to_string(),
            )
            .set(
                "enable.auto.commit",
                if self.enable_auto_commit {
                    "true"
                } else {
                    "false"
                },
            );

        if let Some(ref mechanism) = self.sasl_mechanism {
            config.set("sasl.mechanism", mechanism);
        }
        if let Some(ref username) = self.sasl_username {
            config.set("sasl.username", username);
        }
        if let Some(ref password) = self.sasl_password {
            config.set("sasl.password", password);
        }

        for (k, v) in &self.overrides {
            config.set(k, v);
        }

        config
    }

    /// Build an rdkafka `ClientConfig` for the admin client.
    pub(crate) fn to_admin_client_config(&self) -> rdkafka::ClientConfig {
        let mut config = rdkafka::ClientConfig::new();
        config
            .set("bootstrap.servers", &self.bootstrap_servers)
            .set("security.protocol", self.security_protocol.as_str());

        if let Some(ref mechanism) = self.sasl_mechanism {
            config.set("sasl.mechanism", mechanism);
        }
        if let Some(ref username) = self.sasl_username {
            config.set("sasl.username", username);
        }
        if let Some(ref password) = self.sasl_password {
            config.set("sasl.password", password);
        }

        for (k, v) in &self.overrides {
            config.set(k, v);
        }

        config
    }
}

/// Builder for [`KafkaConfig`].
#[derive(Default)]
pub struct KafkaConfigBuilder {
    config: KafkaConfig,
}

impl KafkaConfigBuilder {
    pub fn bootstrap_servers(mut self, servers: impl Into<String>) -> Self {
        self.config.bootstrap_servers = servers.into();
        self
    }

    pub fn group_id(mut self, group_id: impl Into<String>) -> Self {
        self.config.group_id = group_id.into();
        self
    }

    pub fn security_protocol(mut self, protocol: SecurityProtocol) -> Self {
        self.config.security_protocol = protocol;
        self
    }

    pub fn sasl_mechanism(mut self, mechanism: impl Into<String>) -> Self {
        self.config.sasl_mechanism = Some(mechanism.into());
        self
    }

    pub fn sasl_username(mut self, username: impl Into<String>) -> Self {
        self.config.sasl_username = Some(username.into());
        self
    }

    pub fn sasl_password(mut self, password: impl Into<String>) -> Self {
        self.config.sasl_password = Some(password.into());
        self
    }

    pub fn compression(mut self, compression: Compression) -> Self {
        self.config.compression = compression;
        self
    }

    pub fn acks(mut self, acks: Acks) -> Self {
        self.config.acks = acks;
        self
    }

    pub fn auto_create(mut self, auto_create: bool) -> Self {
        self.config.auto_create = auto_create;
        self
    }

    pub fn default_partitions(mut self, partitions: i32) -> Self {
        self.config.default_partitions = partitions;
        self
    }

    pub fn default_replication_factor(mut self, factor: i32) -> Self {
        self.config.default_replication_factor = factor;
        self
    }

    pub fn session_timeout_ms(mut self, timeout: u32) -> Self {
        self.config.session_timeout_ms = timeout;
        self
    }

    pub fn enable_auto_commit(mut self, enable: bool) -> Self {
        self.config.enable_auto_commit = enable;
        self
    }

    pub fn override_config(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.config.overrides.insert(key.into(), value.into());
        self
    }

    pub fn build(self) -> KafkaConfig {
        self.config
    }
}
