use r2e_events_iggy::{sanitize_topic_name, IggyConfig, IggyEventBus, Transport};

// ── Topic name sanitization ──────────────────────────────────────────

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

// ── IggyConfig builder ──────────────────────────────────────────────

#[test]
fn config_defaults() {
    let config = IggyConfig::default();
    assert_eq!(config.address, "127.0.0.1:8090");
    assert_eq!(config.stream_name, "r2e-events");
    assert_eq!(config.consumer_group, "r2e-app");
    assert!(config.auto_create);
    assert_eq!(config.default_partitions, 1);
    assert_eq!(config.poll_batch_size, 100);
    assert_eq!(
        config.poll_interval,
        std::time::Duration::from_millis(100)
    );
    assert!(config.username.is_none());
    assert!(config.password.is_none());
}

#[test]
fn config_builder() {
    let config = IggyConfig::builder()
        .address("10.0.0.1:9090")
        .stream_name("my-stream")
        .consumer_group("my-group")
        .username("user")
        .password("pass")
        .poll_batch_size(50)
        .default_partitions(4)
        .auto_create(false)
        .build();

    assert_eq!(config.address, "10.0.0.1:9090");
    assert_eq!(config.stream_name, "my-stream");
    assert_eq!(config.consumer_group, "my-group");
    assert_eq!(config.username.as_deref(), Some("user"));
    assert_eq!(config.password.as_deref(), Some("pass"));
    assert_eq!(config.poll_batch_size, 50);
    assert_eq!(config.default_partitions, 4);
    assert!(!config.auto_create);
}

#[test]
fn transport_variants() {
    let _tcp = Transport::Tcp;
    let _quic = Transport::Quic;
    let _http = Transport::Http;
    let _default = Transport::default();
    // Default should be Tcp
    assert!(matches!(Transport::default(), Transport::Tcp));
}

// ── Serialization round-trip ─────────────────────────────────────────

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

// ── Compile-time assertions ──────────────────────────────────────────

#[test]
fn iggy_event_bus_is_clone_send_sync() {
    fn assert_clone_send_sync<T: Clone + Send + Sync + 'static>() {}
    assert_clone_send_sync::<IggyEventBus>();
}

// ── Metadata ↔ headers round-trip ────────────────────────────────────

#[test]
fn metadata_roundtrip() {
    use r2e_events::EventMetadata;

    let metadata = EventMetadata::new()
        .with_correlation_id("corr-123")
        .with_partition_key("user-42")
        .with_header("source", "test");

    // Verify fields are set
    assert_eq!(metadata.correlation_id.as_deref(), Some("corr-123"));
    assert_eq!(metadata.partition_key.as_deref(), Some("user-42"));
    assert_eq!(metadata.headers.get("source").map(|s| s.as_str()), Some("test"));
}

// ── Error bridging ───────────────────────────────────────────────────

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
