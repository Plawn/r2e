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
    let pairs: Vec<_> = encode_metadata(&metadata).collect();

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

// -- Request-reply topic naming ---------------------------------------------

#[test]
fn request_topic_appends_suffix() {
    use r2e_events::backend::request_topic;
    assert_eq!(request_topic("user-created"), "user-created.requests");
}

#[test]
fn request_topic_full_name_carries_prefix() {
    // The responder consumes the fully-qualified request topic.
    use r2e_events::backend::request_topic;
    let config = PulsarConfig::default();
    let full = config.full_topic_name(&request_topic("orders"));
    assert_eq!(full, "persistent://public/default/orders.requests");
}

#[test]
fn reply_topic_is_per_instance_and_stable() {
    use r2e_events::backend::reply_topic;
    let a = reply_topic("r2e-app", 0x1234);
    let b = reply_topic("r2e-app", 0x1234);
    // Same instance id → identical reply topic; carries the `.replies.` segment.
    assert_eq!(a, b);
    assert!(a.starts_with("r2e-app.replies."));
    // Suffix is a 16-hex instance id.
    let hex = a.strip_prefix("r2e-app.replies.").unwrap();
    assert_eq!(hex.len(), 16);
    assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
    // Distinct instance ids → distinct reply topics, so two bus instances
    // sharing a subscription in one process never collide on the same topic.
    assert_ne!(reply_topic("r2e-app", 1), reply_topic("r2e-app", 2));
}

#[test]
fn reply_topic_full_name_carries_prefix() {
    use r2e_events::backend::reply_topic;
    let config = PulsarConfig::default();
    let full = config.full_topic_name(&reply_topic(&config.subscription, 0xABCD));
    assert!(full.starts_with("persistent://public/default/r2e-app.replies."));
}

// -- Reply header round-trip ------------------------------------------------

#[test]
fn reply_headers_request_roundtrip() {
    use r2e_events::backend::{decode_reply_headers, encode_reply_headers};

    // A request carries a request id + reply-to (no error).
    let pairs: Vec<_> =
        encode_reply_headers(0xABCD_1234, Some("persistent://x/replies.00"), None).collect();
    let decoded = decode_reply_headers(pairs.iter().map(|(k, v)| (k, v))).unwrap();

    assert_eq!(decoded.request_id, 0xABCD_1234);
    assert_eq!(decoded.reply_to.as_deref(), Some("persistent://x/replies.00"));
    assert!(decoded.reply_error.is_none());
}

#[test]
fn reply_headers_error_reply_roundtrip() {
    use r2e_events::backend::{decode_reply_headers, encode_reply_headers};

    // An error reply echoes the request id and carries the error payload.
    let pairs: Vec<_> = encode_reply_headers(99, None, Some("boom")).collect();
    let decoded = decode_reply_headers(pairs.iter().map(|(k, v)| (k, v))).unwrap();

    assert_eq!(decoded.request_id, 99);
    assert!(decoded.reply_to.is_none());
    assert_eq!(decoded.reply_error.as_deref(), Some("boom"));
}

#[test]
fn reply_headers_absent_without_correlation_id() {
    use r2e_events::backend::decode_reply_headers;
    // A plain event (no correlation id) is not a request-reply exchange.
    let pairs: Vec<(String, String)> = vec![("r2e-h-source".into(), "test".into())];
    assert!(decode_reply_headers(pairs.iter().map(|(k, v)| (k, v))).is_none());
}

// -- Responder registration (one per request type) --------------------------

#[tokio::test]
async fn register_responder_rejects_duplicate() {
    use r2e_events::backend::{BackendState, TopicRegistry};
    use r2e_events::{EventBusError, EventEnvelope};
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize)]
    struct Ping(u32);
    #[derive(Serialize, Deserialize)]
    struct Pong(u32);

    let state = BackendState::new(TopicRegistry::default());

    let first = state
        .register_responder::<Ping, Pong, String, _, _>(|env: EventEnvelope<Ping>| async move {
            Ok(Pong(env.event.0))
        })
        .await;
    assert!(first.is_ok());

    // A second responder for the same request type is rejected.
    let second = state
        .register_responder::<Ping, Pong, String, _, _>(|env: EventEnvelope<Ping>| async move {
            Ok(Pong(env.event.0))
        })
        .await;
    assert!(matches!(second, Err(EventBusError::Other(_))));
}

#[cfg(feature = "integration")]
#[tokio::test(flavor = "multi_thread")]
async fn live_broker_request_reply_roundtrip() {
    use r2e_events::{EventBus, EventEnvelope, RequestOptions};
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize)]
    struct Ping(u32);
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Pong(u32);

    let nonce = r2e_events::backend::instance_id();
    let topic = format!("r2e.integration.pulsar.{nonce:016x}");
    let config = PulsarConfig::builder()
        .subscription(format!("r2e-integration-{nonce:016x}"))
        .build();
    let bus = PulsarEventBus::builder(config).topic::<Ping>(topic).connect().await
        .expect("Pulsar broker must be available for integration tests");
    let _responder = bus.respond(|env: EventEnvelope<Ping>| async move {
        Ok::<_, String>(Pong(env.event.0 + 1))
    }).await.unwrap();
    let reply: Pong = bus.request_with(
        Ping(41),
        RequestOptions::new().with_timeout(std::time::Duration::from_secs(20)),
    ).await.unwrap();
    assert_eq!(reply, Pong(42));
    bus.shutdown(std::time::Duration::from_secs(10)).await.unwrap();
}
