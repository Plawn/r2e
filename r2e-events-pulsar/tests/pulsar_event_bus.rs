use r2e_events_pulsar::{sanitize_topic_name, PulsarConfig, PulsarEventBus, SubscriptionType};

// -- Topic name sanitization ------------------------------------------------

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

// -- PulsarConfig defaults --------------------------------------------------

#[test]
fn config_defaults() {
    let config = PulsarConfig::default();
    assert_eq!(config.service_url, "pulsar://localhost:6650");
    assert_eq!(config.subscription, "r2e-app");
    assert_eq!(config.topic_prefix, "persistent://public/default/");
    assert!(config.auto_create);
    assert_eq!(config.default_partitions, 0);
    assert_eq!(config.batch_size, 100);
    assert!(!config.tls_hostname_verification);
    assert!(config.auth_token.is_none());
    assert!(matches!(
        config.subscription_type,
        SubscriptionType::Shared
    ));
}

// -- PulsarConfig builder ---------------------------------------------------

#[test]
fn config_builder() {
    let config = PulsarConfig::builder()
        .service_url("pulsar://10.0.0.1:6651")
        .subscription("my-group")
        .subscription_type(SubscriptionType::KeyShared)
        .topic_prefix("persistent://my-tenant/my-ns/")
        .auth_token("jwt-token-here")
        .tls_hostname_verification(true)
        .batch_size(50)
        .default_partitions(4)
        .auto_create(false)
        .build();

    assert_eq!(config.service_url, "pulsar://10.0.0.1:6651");
    assert_eq!(config.subscription, "my-group");
    assert!(matches!(
        config.subscription_type,
        SubscriptionType::KeyShared
    ));
    assert_eq!(config.topic_prefix, "persistent://my-tenant/my-ns/");
    assert_eq!(config.auth_token.as_deref(), Some("jwt-token-here"));
    assert!(config.tls_hostname_verification);
    assert_eq!(config.batch_size, 50);
    assert_eq!(config.default_partitions, 4);
    assert!(!config.auto_create);
}

// -- SubscriptionType variants ----------------------------------------------

#[test]
fn subscription_type_variants() {
    let _shared = SubscriptionType::Shared;
    let _exclusive = SubscriptionType::Exclusive;
    let _failover = SubscriptionType::Failover;
    let _key_shared = SubscriptionType::KeyShared;
    let _default = SubscriptionType::default();
    // Default should be Shared
    assert!(matches!(SubscriptionType::default(), SubscriptionType::Shared));
}

// -- Full topic name --------------------------------------------------------

#[test]
fn full_topic_name() {
    let config = PulsarConfig::default();
    assert_eq!(
        config.full_topic_name("user-created"),
        "persistent://public/default/user-created"
    );
}

#[test]
fn full_topic_name_custom_prefix() {
    let config = PulsarConfig::builder()
        .topic_prefix("persistent://my-tenant/my-ns/")
        .build();
    assert_eq!(
        config.full_topic_name("orders"),
        "persistent://my-tenant/my-ns/orders"
    );
}

// -- Serialization round-trip -----------------------------------------------

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

// -- Compile-time assertions ------------------------------------------------

#[test]
fn pulsar_event_bus_is_clone_send_sync() {
    fn assert_clone_send_sync<T: Clone + Send + Sync + 'static>() {}
    assert_clone_send_sync::<PulsarEventBus>();
}

// -- Metadata round-trip ----------------------------------------------------

#[test]
fn metadata_roundtrip() {
    use r2e_events::backend::{decode_metadata, encode_metadata};
    use r2e_events::EventMetadata;

    let metadata = EventMetadata::new()
        .with_correlation_id("corr-123")
        .with_partition_key("user-42")
        .with_header("source", "test");

    // Encode to pairs (as would be stored in Pulsar message properties)
    let pairs = encode_metadata(&metadata);

    // Decode back
    let decoded = decode_metadata(pairs.into_iter());

    assert_eq!(decoded.event_id, metadata.event_id);
    assert_eq!(decoded.correlation_id.as_deref(), Some("corr-123"));
    assert_eq!(decoded.partition_key.as_deref(), Some("user-42"));
    assert_eq!(
        decoded.headers.get("source").map(|s| s.as_str()),
        Some("test")
    );
}

#[test]
fn metadata_fields_set() {
    use r2e_events::EventMetadata;

    let metadata = EventMetadata::new()
        .with_correlation_id("corr-456")
        .with_partition_key("order-99")
        .with_header("region", "us-east");

    assert_eq!(metadata.correlation_id.as_deref(), Some("corr-456"));
    assert_eq!(metadata.partition_key.as_deref(), Some("order-99"));
    assert_eq!(
        metadata.headers.get("region").map(|s| s.as_str()),
        Some("us-east")
    );
}

// -- Error display ----------------------------------------------------------

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

// -- Topic sanitization via shared backend ----------------------------------

#[test]
fn sanitize_nested_module_path() {
    assert_eq!(
        sanitize_topic_name("app::domain::events::OrderPlaced"),
        "app.domain.events.OrderPlaced"
    );
}

#[test]
fn sanitize_preserves_hyphens_and_underscores() {
    assert_eq!(sanitize_topic_name("my-topic_name"), "my-topic_name");
}
