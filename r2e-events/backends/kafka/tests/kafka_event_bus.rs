use r2e_events_kafka::{Acks, Compression, KafkaConfig, KafkaEventBus, SecurityProtocol};

// ── Config defaults ─────────────────────────────────────────────────

#[test]
fn config_defaults() {
    let config = KafkaConfig::default();
    assert_eq!(config.bootstrap_servers, "localhost:9092");
    assert_eq!(config.group_id, "r2e-app");
    assert!(config.auto_create);
    assert_eq!(config.default_partitions, 1);
    assert_eq!(config.default_replication_factor, 1);
    assert_eq!(config.session_timeout_ms, 30000);
    assert!(config.enable_auto_commit);
    assert!(config.sasl_mechanism.is_none());
    assert!(config.sasl_username.is_none());
    assert!(config.sasl_password.is_none());
    assert!(config.overrides.is_empty());
}

#[test]
fn config_builder() {
    let config = KafkaConfig::builder()
        .bootstrap_servers("10.0.0.1:9092,10.0.0.2:9092")
        .group_id("my-group")
        .security_protocol(SecurityProtocol::SaslSsl)
        .sasl_mechanism("SCRAM-SHA-256")
        .sasl_username("user")
        .sasl_password("pass")
        .compression(Compression::Zstd)
        .acks(Acks::One)
        .auto_create(false)
        .default_partitions(4)
        .default_replication_factor(3)
        .session_timeout_ms(10000)
        .enable_auto_commit(false)
        .override_config("fetch.min.bytes", "1024")
        .build();

    assert_eq!(config.bootstrap_servers, "10.0.0.1:9092,10.0.0.2:9092");
    assert_eq!(config.group_id, "my-group");
    assert_eq!(config.sasl_mechanism.as_deref(), Some("SCRAM-SHA-256"));
    assert_eq!(config.sasl_username.as_deref(), Some("user"));
    assert_eq!(config.sasl_password.as_deref(), Some("pass"));
    assert!(!config.auto_create);
    assert_eq!(config.default_partitions, 4);
    assert_eq!(config.default_replication_factor, 3);
    assert_eq!(config.session_timeout_ms, 10000);
    assert!(!config.enable_auto_commit);
    assert_eq!(
        config.overrides.get("fetch.min.bytes").map(|s| s.as_str()),
        Some("1024")
    );
}

// ── Enum variants ───────────────────────────────────────────────────

#[test]
fn security_protocol_variants() {
    let _plain = SecurityProtocol::Plaintext;
    let _ssl = SecurityProtocol::Ssl;
    let _sasl = SecurityProtocol::SaslPlaintext;
    let _sasl_ssl = SecurityProtocol::SaslSsl;
    assert!(matches!(SecurityProtocol::default(), SecurityProtocol::Plaintext));
}

#[test]
fn compression_variants() {
    let _none = Compression::None;
    let _gzip = Compression::Gzip;
    let _snappy = Compression::Snappy;
    let _lz4 = Compression::Lz4;
    let _zstd = Compression::Zstd;
    assert!(matches!(Compression::default(), Compression::None));
}

#[test]
fn acks_variants() {
    let _zero = Acks::Zero;
    let _one = Acks::One;
    let _all = Acks::All;
    assert!(matches!(Acks::default(), Acks::All));
}

// ── Compile-time assertions ─────────────────────────────────────────

#[test]
fn kafka_event_bus_is_clone_send_sync() {
    fn assert_clone_send_sync<T: Clone + Send + Sync + 'static>() {}
    assert_clone_send_sync::<KafkaEventBus>();
}

// ── Serialization round-trip ────────────────────────────────────────

#[test]
fn json_roundtrip() {
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct TestEvent {
        id: u64,
        name: String,
    }

    let event = TestEvent {
        id: 42,
        name: "Alice".into(),
    };

    let bytes = serde_json::to_vec(&event).unwrap();
    let decoded: TestEvent = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(event, decoded);
}

// ── Metadata round-trip ─────────────────────────────────────────────

#[test]
fn metadata_roundtrip() {
    use r2e_events::backend::{decode_metadata, encode_metadata};
    use r2e_events::EventMetadata;

    let metadata = EventMetadata::new()
        .with_correlation_id("corr-123")
        .with_partition_key("user-42")
        .with_header("source", "test");

    let pairs = encode_metadata(&metadata);
    let decoded = decode_metadata(pairs.into_iter());

    assert_eq!(decoded.event_id, metadata.event_id);
    assert_eq!(decoded.timestamp, metadata.timestamp);
    assert_eq!(decoded.correlation_id.as_deref(), Some("corr-123"));
    assert_eq!(decoded.partition_key.as_deref(), Some("user-42"));
    assert_eq!(
        decoded.headers.get("source").map(|s| s.as_str()),
        Some("test")
    );
}

// ── Topic sanitization ──────────────────────────────────────────────

#[test]
fn topic_sanitization() {
    use r2e_events::backend::sanitize_topic_name;

    assert_eq!(
        sanitize_topic_name("my_crate::events::UserCreated"),
        "my_crate.events.UserCreated"
    );
    assert_eq!(
        sanitize_topic_name("my_crate::Wrapper<u32>"),
        "my_crate.Wrapper_u32"
    );
    assert_eq!(sanitize_topic_name("Foo"), "Foo");
}

// ── Error bridging ──────────────────────────────────────────────────

#[test]
fn error_display() {
    use r2e_events::EventBusError;

    let err = EventBusError::Connection("timeout".into());
    assert!(err.to_string().contains("timeout"));

    let err = EventBusError::Serialization("bad json".into());
    assert!(err.to_string().contains("bad json"));

    let err = EventBusError::Shutdown;
    assert!(err.to_string().contains("shut down"));
}

// ── Config builder doesn't panic ────────────────────────────────────

#[test]
fn config_builder_roundtrip() {
    // Building a config with all options shouldn't panic
    let _config = KafkaConfig::builder()
        .bootstrap_servers("localhost:9092")
        .group_id("test")
        .security_protocol(SecurityProtocol::SaslSsl)
        .sasl_mechanism("PLAIN")
        .sasl_username("u")
        .sasl_password("p")
        .compression(Compression::Gzip)
        .acks(Acks::Zero)
        .auto_create(true)
        .default_partitions(3)
        .default_replication_factor(2)
        .session_timeout_ms(5000)
        .enable_auto_commit(false)
        .override_config("key", "value")
        .build();
}
