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
    assert_eq!(config.default_partitions, 3);
    assert_eq!(config.poll_batch_size, 1000);
    assert_eq!(
        config.poll_interval,
        std::time::Duration::from_millis(10)
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

// ── Request-reply topic naming ───────────────────────────────────────

#[test]
fn request_topic_derives_from_event_topic() {
    use r2e_events::backend::request_topic;

    assert_eq!(
        request_topic("orders.OrderPlaced"),
        "orders.OrderPlaced.requests"
    );
}

#[test]
fn reply_topic_is_instance_scoped() {
    use r2e_events::backend::{instance_id, reply_topic};

    let iid = instance_id();
    let a = reply_topic("r2e-app", iid);
    // `<prefix>.replies.<instance-id-hex>` — prefixed and instance-private.
    assert!(a.starts_with("r2e-app.replies."));
    // Stable for the same instance id.
    assert_eq!(a, reply_topic("r2e-app", iid));
    // Distinct instance ids yield distinct reply topics (so two bus instances
    // in one process never cross-consume each other's replies).
    assert_ne!(
        reply_topic("r2e-app", iid),
        reply_topic("r2e-app", iid.wrapping_add(1))
    );
    // Distinct prefixes yield distinct reply topics.
    assert_ne!(reply_topic("a", iid), reply_topic("b", iid));
}

// ── Request-reply header codec (Iggy round-trip) ─────────────────────

#[test]
fn reply_headers_survive_iggy_message_roundtrip() {
    use iggy::prelude::IggyMessage;
    use r2e_events::backend::{decode_metadata, encode_metadata, encode_reply_headers};
    use r2e_events::EventMetadata;
    use r2e_events_iggy::{headers_from_pairs, reply_headers_from_message};

    // Mirror the request path (bus.rs `request_with`): the user's metadata —
    // including its own `correlation_id` string — is encoded alongside the
    // internal request-reply headers, which live in a dedicated header slot.
    let request_id: u128 = 0x1234_5678_9abc_def0_dead_beef_cafe_babe;
    let metadata = EventMetadata::new()
        .with_correlation_id("user-corr-9")
        .with_header("source", "test");
    let pairs = encode_metadata(&metadata).chain(encode_reply_headers(
        request_id,
        Some("r2e-app.replies.00ff"),
        None,
    ));
    let headers = headers_from_pairs(pairs).expect("valid iggy headers");

    let msg = IggyMessage::builder()
        .payload(bytes::Bytes::from_static(b"{}"))
        .user_headers(headers)
        .build()
        .expect("build message");

    // The internal request-reply id survives in its own slot.
    let decoded = reply_headers_from_message(&msg).expect("reply headers present");
    assert_eq!(decoded.request_id, request_id);
    assert_eq!(decoded.reply_to.as_deref(), Some("r2e-app.replies.00ff"));
    assert!(decoded.reply_error.is_none());

    // The user's metadata.correlation_id survives the round-trip untouched —
    // the internal request id never overwrites it.
    let back_pairs: Vec<(String, String)> = msg
        .user_headers_map()
        .expect("headers readable")
        .expect("headers present")
        .iter()
        .filter_map(|(k, v)| Some((k.as_str().ok()?.to_string(), v.as_str().ok()?.to_string())))
        .collect();
    let meta = decode_metadata(back_pairs.into_iter());
    assert_eq!(meta.correlation_id.as_deref(), Some("user-corr-9"));
    assert_eq!(meta.headers.get("source").map(|s| s.as_str()), Some("test"));
}

#[test]
fn reply_error_header_survives_roundtrip() {
    use iggy::prelude::IggyMessage;
    use r2e_events::backend::encode_reply_headers;
    use r2e_events_iggy::{headers_from_pairs, reply_headers_from_message};

    let request_id: u128 = 42;
    let pairs = encode_reply_headers(request_id, None, Some("boom"));
    let headers = headers_from_pairs(pairs).expect("valid iggy headers");

    // Error replies carry a non-empty placeholder payload (Iggy rejects empty).
    let msg = IggyMessage::builder()
        .payload(bytes::Bytes::from_static(b"null"))
        .user_headers(headers)
        .build()
        .expect("build message");

    let decoded = reply_headers_from_message(&msg).expect("reply headers present");
    assert_eq!(decoded.request_id, request_id);
    assert_eq!(decoded.reply_error.as_deref(), Some("boom"));
}

#[test]
fn plain_message_has_no_reply_headers() {
    use iggy::prelude::IggyMessage;
    use r2e_events_iggy::reply_headers_from_message;

    // A message with no user headers is not part of a request-reply exchange.
    let msg = IggyMessage::builder()
        .payload(bytes::Bytes::from_static(b"payload"))
        .build()
        .expect("build message");

    assert!(reply_headers_from_message(&msg).is_none());
}
