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
    assert!(matches!(
        SecurityProtocol::default(),
        SecurityProtocol::Plaintext
    ));
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

// ── Request-reply: topic naming ─────────────────────────────────────

#[test]
fn request_and_reply_topic_naming() {
    use r2e_events::backend::{instance_id, reply_topic, request_topic, REQUEST_TOPIC_SUFFIX};

    // The shared request topic derives from the event topic.
    assert_eq!(request_topic("user-created"), "user-created.requests");
    assert_eq!(REQUEST_TOPIC_SUFFIX, ".requests");

    // The reply topic is instance-private: prefixed by the group and suffixed
    // with the bus instance's hex nonce, so two instances never share a reply
    // topic — even within one process.
    let id = instance_id();
    let reply = reply_topic("r2e-app", id);
    assert!(reply.starts_with("r2e-app.replies."));
    assert_eq!(
        reply,
        reply_topic("r2e-app", id),
        "reply topic is stable for an instance id"
    );
    // 16 hex chars for the u64 instance nonce.
    let suffix = reply.trim_start_matches("r2e-app.replies.");
    assert_eq!(suffix.len(), 16);
    assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()));
}

// ── Request-reply: reply-header round-trip (produce/consume encode) ──

#[test]
fn reply_header_roundtrip_ok() {
    use r2e_events::backend::{decode_reply_headers, encode_reply_headers};

    // A successful reply: correlation id echoed, no reply-to, no error.
    let pairs: Vec<_> =
        encode_reply_headers(0x1234_5678_9abc_def0_1111_2222_3333_4444, None, None).collect();
    // Simulate the produce -> consume header path (UTF-8 string values).
    let decoded = decode_reply_headers(pairs.iter().map(|(k, v)| (k.as_ref(), v.as_str())))
        .expect("request id present");
    assert_eq!(
        decoded.request_id,
        0x1234_5678_9abc_def0_1111_2222_3333_4444
    );
    assert!(decoded.reply_to.is_none());
    assert!(decoded.reply_error.is_none());
}

#[test]
fn reply_header_roundtrip_error() {
    use r2e_events::backend::{decode_reply_headers, encode_reply_headers};

    // A failed reply carries the remote-error payload.
    let pairs: Vec<_> = encode_reply_headers(99, None, Some("boom")).collect();
    let decoded = decode_reply_headers(pairs.iter().map(|(k, v)| (k.as_ref(), v.as_str())))
        .expect("request id present");
    assert_eq!(decoded.request_id, 99);
    assert_eq!(decoded.reply_error.as_deref(), Some("boom"));
}

#[test]
fn request_id_is_separate_from_user_correlation_id() {
    // Mirrors `KafkaEventBus::publish_request`: the u128 request id lives in its
    // own dedicated header slot, so the user's `metadata.correlation_id` flows
    // through the correlation-id slot untouched — the two never collide and the
    // request path no longer strips the user's correlation id.
    use r2e_events::backend::{
        decode_metadata, decode_reply_headers, encode_metadata, encode_reply_headers,
        HEADER_CORRELATION_ID, HEADER_REQUEST_ID,
    };
    use r2e_events::EventMetadata;

    let metadata = EventMetadata::new()
        .with_correlation_id("trace-abc")
        .with_partition_key("user-7")
        .with_header("source", "svc");
    let request_id: u128 = 0xaaaa_bbbb_cccc_dddd;
    let reply_to = "r2e-app.replies.00000000cafef00d";

    let pairs: Vec<_> = encode_metadata(&metadata)
        .chain(encode_reply_headers(request_id, Some(reply_to), None))
        .collect();

    // The request id rides its own slot; the user's correlation id rides its own.
    assert_eq!(
        pairs.iter().filter(|(k, _)| k == HEADER_REQUEST_ID).count(),
        1,
        "exactly one request-id header"
    );
    assert_eq!(
        pairs
            .iter()
            .filter(|(k, _)| k == HEADER_CORRELATION_ID)
            .count(),
        1,
        "the user's correlation id flows through untouched"
    );

    let reply = decode_reply_headers(pairs.iter().map(|(k, v)| (k.as_ref(), v.as_str())))
        .expect("request id present");
    assert_eq!(reply.request_id, request_id);
    assert_eq!(reply.reply_to.as_deref(), Some(reply_to));

    // Metadata round-trips ALL its fields, including the correlation id.
    let decoded_meta = decode_metadata(pairs.into_iter());
    assert_eq!(decoded_meta.correlation_id.as_deref(), Some("trace-abc"));
    assert_eq!(decoded_meta.event_id, metadata.event_id);
    assert_eq!(decoded_meta.partition_key.as_deref(), Some("user-7"));
    assert_eq!(
        decoded_meta.headers.get("source").map(|s| s.as_str()),
        Some("svc")
    );
}

// ── Request-reply: responder registration (at most one per type) ────

#[tokio::test]
async fn responder_registration_is_unique_per_type() {
    use r2e_events::backend::{BackendState, TopicRegistry};
    use r2e_events::EventEnvelope;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize)]
    struct Ping {
        n: u32,
    }
    #[derive(Serialize, Deserialize)]
    struct Pong {
        n: u32,
    }

    let state = BackendState::new(TopicRegistry::default());

    let first = state
        .register_responder::<Ping, Pong, String, _, _>(|env: EventEnvelope<Ping>| async move {
            Ok(Pong { n: env.event.n + 1 })
        })
        .await;
    assert!(first.is_ok());

    // A second responder for the same request type is rejected.
    let second = state
        .register_responder::<Ping, Pong, String, _, _>(|env: EventEnvelope<Ping>| async move {
            Ok(Pong { n: env.event.n })
        })
        .await;
    assert!(second.is_err(), "duplicate responder must error");
}

#[tokio::test]
async fn invoke_responder_round_trips_bytes_and_absence() {
    use r2e_events::backend::{BackendState, TopicRegistry};
    use r2e_events::{EventEnvelope, EventMetadata};
    use serde::{Deserialize, Serialize};
    use std::any::TypeId;

    #[derive(Serialize, Deserialize)]
    struct Req {
        v: i64,
    }
    #[derive(Serialize, Deserialize)]
    struct Resp {
        v: i64,
    }

    let state = BackendState::new(TopicRegistry::default());
    state
        .register_responder::<Req, Resp, String, _, _>(|env: EventEnvelope<Req>| async move {
            if env.event.v < 0 {
                Err("negative".to_string())
            } else {
                Ok(Resp { v: env.event.v * 2 })
            }
        })
        .await
        .unwrap();

    let type_id = TypeId::of::<Req>();

    // Ok path: reply bytes deserialize to the doubled value.
    let payload = serde_json::to_vec(&Req { v: 21 }).unwrap();
    let out = state
        .invoke_responder(type_id, &payload, EventMetadata::new())
        .await
        .expect("responder registered");
    let bytes = out.expect("responder ok");
    let resp: Resp = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(resp.v, 42);

    // Err path surfaces the remote-error message.
    let payload = serde_json::to_vec(&Req { v: -1 }).unwrap();
    let out = state
        .invoke_responder(type_id, &payload, EventMetadata::new())
        .await
        .expect("responder registered");
    assert_eq!(out.unwrap_err(), "negative");

    // Absent responder -> None (the responder consumer maps this to a
    // no-responder reply-error).
    let absent = state
        .invoke_responder(TypeId::of::<Resp>(), &[], EventMetadata::new())
        .await;
    assert!(absent.is_none());
}

// ── Request-reply: pending correlation map ──────────────────────────

#[tokio::test]
async fn pending_request_completes_by_correlation_id() {
    use r2e_events::backend::PendingRequests;
    use std::sync::Arc;

    let pending = Arc::new(PendingRequests::new());
    let (id, _guard, rx) = pending.register();
    assert_eq!(pending.len(), 1);

    pending.complete(id, Ok(b"reply-bytes".to_vec()));
    let got = rx.await.unwrap().unwrap();
    assert_eq!(got, b"reply-bytes");
    assert_eq!(pending.len(), 0, "completed entry is removed");
}

#[tokio::test]
async fn pending_guard_drop_evicts_entry() {
    use r2e_events::backend::PendingRequests;
    use std::sync::Arc;

    let pending = Arc::new(PendingRequests::new());
    let (_id, guard, _rx) = pending.register();
    assert_eq!(pending.len(), 1);
    // Dropping the guard (as on a request timeout) evicts the entry so a late
    // reply is discarded rather than leaking a map slot.
    drop(guard);
    assert_eq!(pending.len(), 0);
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
    let topic = format!("r2e.integration.kafka.{nonce:016x}");
    let config = KafkaConfig::builder()
        .group_id(format!("r2e-integration-{nonce:016x}"))
        .build();
    let bus = KafkaEventBus::builder(config)
        .topic::<Ping>(topic)
        .connect()
        .await
        .expect("Kafka broker must be available for integration tests");
    let _responder = bus
        .respond(|env: EventEnvelope<Ping>| async move { Ok::<_, String>(Pong(env.event.0 + 1)) })
        .await
        .unwrap();
    let reply: Pong = bus
        .request_with(
            Ping(41),
            RequestOptions::new().with_timeout(std::time::Duration::from_secs(20)),
        )
        .await
        .unwrap();
    assert_eq!(reply, Pong(42));
    bus.shutdown(std::time::Duration::from_secs(10))
        .await
        .unwrap();
}
