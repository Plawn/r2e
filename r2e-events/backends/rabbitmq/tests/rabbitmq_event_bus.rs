use r2e_events_rabbitmq::{sanitize_topic_name, RabbitMqConfig, RabbitMqEventBus};

// ── Config defaults ──────────────────────────────────────────────────

#[test]
fn config_defaults() {
    let config = RabbitMqConfig::default();
    assert_eq!(config.uri, "amqp://guest:guest@localhost:5672/%2f");
    assert_eq!(config.exchange, "r2e-events");
    assert_eq!(config.consumer_group, "r2e-app");
    assert_eq!(config.prefetch_count, 10);
    assert!(config.durable);
    assert!(config.persistent);
    assert!(config.auto_create);
    assert!(config.message_ttl_ms.is_none());
    assert!(config.dead_letter_exchange.is_none());
    assert_eq!(config.heartbeat, 60);
    assert!(config.connection_name.is_none());
}

// ── Config builder ───────────────────────────────────────────────────

#[test]
fn config_builder() {
    let config = RabbitMqConfig::builder()
        .uri("amqp://user:pass@rabbitmq:5672/vhost")
        .exchange("my-exchange")
        .consumer_group("my-service")
        .prefetch_count(50)
        .durable(false)
        .persistent(false)
        .auto_create(false)
        .message_ttl_ms(30000)
        .dead_letter_exchange("dlx")
        .heartbeat(30)
        .connection_name("my-conn")
        .build();

    assert_eq!(config.uri, "amqp://user:pass@rabbitmq:5672/vhost");
    assert_eq!(config.exchange, "my-exchange");
    assert_eq!(config.consumer_group, "my-service");
    assert_eq!(config.prefetch_count, 50);
    assert!(!config.durable);
    assert!(!config.persistent);
    assert!(!config.auto_create);
    assert_eq!(config.message_ttl_ms, Some(30000));
    assert_eq!(config.dead_letter_exchange.as_deref(), Some("dlx"));
    assert_eq!(config.heartbeat, 30);
    assert_eq!(config.connection_name.as_deref(), Some("my-conn"));
}

// ── Compile-time assertions ──────────────────────────────────────────

#[test]
fn rabbitmq_event_bus_is_clone_send_sync() {
    fn assert_clone_send_sync<T: Clone + Send + Sync + 'static>() {}
    assert_clone_send_sync::<RabbitMqEventBus>();
}

// ── JSON round-trip ──────────────────────────────────────────────────

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

// ── Metadata round-trip ──────────────────────────────────────────────

#[test]
fn metadata_roundtrip() {
    use r2e_events::backend::{decode_metadata, encode_metadata};
    use r2e_events::EventMetadata;

    let metadata = EventMetadata::new()
        .with_correlation_id("corr-456")
        .with_partition_key("user-99")
        .with_header("source", "rabbitmq-test");

    let pairs = encode_metadata(&metadata);
    let decoded = decode_metadata(pairs.into_iter());

    assert_eq!(decoded.event_id, metadata.event_id);
    assert_eq!(decoded.timestamp, metadata.timestamp);
    assert_eq!(decoded.correlation_id.as_deref(), Some("corr-456"));
    assert_eq!(decoded.partition_key.as_deref(), Some("user-99"));
    assert_eq!(
        decoded.headers.get("source").map(|s| s.as_str()),
        Some("rabbitmq-test")
    );
}

// ── Error display ────────────────────────────────────────────────────

#[test]
fn error_display() {
    use r2e_events::EventBusError;

    let err = EventBusError::Connection("connection refused".into());
    assert!(err.to_string().contains("connection refused"));

    let err = EventBusError::Serialization("invalid json".into());
    assert!(err.to_string().contains("invalid json"));

    let err = EventBusError::Shutdown;
    assert!(err.to_string().contains("shut down"));

    let err = EventBusError::Other("something went wrong".into());
    assert!(err.to_string().contains("something went wrong"));
}

// ── Topic sanitization ──────────────────────────────────────────────

#[test]
fn sanitize_double_colon() {
    assert_eq!(
        sanitize_topic_name("my_crate::events::UserCreated"),
        "my_crate.events.UserCreated"
    );
}

#[test]
fn sanitize_with_generics() {
    assert_eq!(
        sanitize_topic_name("my_crate::Wrapper<u32>"),
        "my_crate.Wrapper_u32"
    );
}

#[test]
fn sanitize_simple_name() {
    assert_eq!(sanitize_topic_name("Foo"), "Foo");
}
