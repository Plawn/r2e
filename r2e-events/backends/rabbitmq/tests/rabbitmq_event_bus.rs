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
    assert!(config.reconnect);
    assert_eq!(
        config.reconnect_max_backoff,
        std::time::Duration::from_secs(60)
    );
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
        .reconnect(false)
        .reconnect_max_backoff(std::time::Duration::from_secs(5))
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
    assert!(!config.reconnect);
    assert_eq!(
        config.reconnect_max_backoff,
        std::time::Duration::from_secs(5)
    );
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

// ── Request-reply topic / queue naming ───────────────────────────────
//
// The shared request queue is named after the request topic itself (no
// consumer-group prefix), so every instance consumes the one queue and the
// broker load-balances requests. Its name equals `request_topic(<topic>)`.

#[test]
fn request_topic_appends_requests_suffix() {
    use r2e_events::backend::request_topic;

    assert_eq!(request_topic("user-created"), "user-created.requests");
    assert_eq!(
        request_topic("my_crate.events.GetUser"),
        "my_crate.events.GetUser.requests"
    );
}

// ── Responder registration (at most one per type per process) ─────────

mod responder {
    use r2e_events::backend::{BackendState, TopicRegistry};
    use r2e_events::EventEnvelope;
    use serde::{Deserialize, Serialize};
    use std::sync::Arc;

    #[derive(Debug, Serialize, Deserialize)]
    struct GetUser {
        id: u64,
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct User {
        id: u64,
        name: String,
    }

    fn state() -> Arc<BackendState> {
        Arc::new(BackendState::new(TopicRegistry::default()))
    }

    #[tokio::test]
    async fn first_responder_registers_second_errors() {
        let state = state();

        let first = state
            .register_responder::<GetUser, User, _, _>(|env: EventEnvelope<GetUser>| async move {
                Ok(User {
                    id: env.event.id,
                    name: "Alice".into(),
                })
            })
            .await;
        assert!(first.is_ok(), "first responder should register");

        let second = state
            .register_responder::<GetUser, User, _, _>(|env: EventEnvelope<GetUser>| async move {
                Ok(User {
                    id: env.event.id,
                    name: "Bob".into(),
                })
            })
            .await;
        assert!(
            second.is_err(),
            "second responder for the same type must error"
        );
    }

    #[tokio::test]
    async fn unregister_allows_reregistration() {
        let state = state();

        state
            .register_responder::<GetUser, User, _, _>(|_env: EventEnvelope<GetUser>| async move {
                Ok(User {
                    id: 1,
                    name: "Alice".into(),
                })
            })
            .await
            .unwrap();

        state
            .unregister_responder(std::any::TypeId::of::<GetUser>())
            .await;

        let again = state
            .register_responder::<GetUser, User, _, _>(|_env: EventEnvelope<GetUser>| async move {
                Ok(User {
                    id: 2,
                    name: "Bob".into(),
                })
            })
            .await;
        assert!(again.is_ok(), "re-registration after unregister should work");
    }

    #[tokio::test]
    async fn invoke_responder_roundtrips_the_reply() {
        use r2e_events::EventMetadata;

        let state = state();
        state
            .register_responder::<GetUser, User, _, _>(|env: EventEnvelope<GetUser>| async move {
                Ok(User {
                    id: env.event.id,
                    name: format!("user-{}", env.event.id),
                })
            })
            .await
            .unwrap();

        let req = serde_json::to_vec(&GetUser { id: 7 }).unwrap();
        let reply = state
            .invoke_responder(
                std::any::TypeId::of::<GetUser>(),
                &req,
                EventMetadata::new(),
            )
            .await
            .expect("responder is registered")
            .expect("handler succeeds");

        let user: User = serde_json::from_slice(&reply).unwrap();
        assert_eq!(user.id, 7);
        assert_eq!(user.name, "user-7");
    }

    #[tokio::test]
    async fn invoke_responder_absent_returns_none() {
        let state = state();
        let out = state
            .invoke_responder(
                std::any::TypeId::of::<GetUser>(),
                b"{}",
                r2e_events::EventMetadata::new(),
            )
            .await;
        assert!(out.is_none(), "no responder registered → None");
    }
}

// ── Pending request correlation map ──────────────────────────────────

mod pending {
    use r2e_events::backend::PendingRequests;
    use r2e_events::EventBusError;
    use std::sync::Arc;

    #[tokio::test]
    async fn register_then_complete_delivers_reply() {
        let pending = Arc::new(PendingRequests::new());
        let (id, _guard, rx) = pending.register();
        assert_eq!(pending.len(), 1);

        pending.complete(id, Ok(b"hello".to_vec()));
        let reply = rx.await.unwrap().unwrap();
        assert_eq!(reply, b"hello");
        // The entry is removed once completed.
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn remote_error_propagates() {
        let pending = Arc::new(PendingRequests::new());
        let (id, _guard, rx) = pending.register();

        pending.complete(id, Err(EventBusError::Remote("boom".into())));
        match rx.await.unwrap() {
            Err(EventBusError::Remote(msg)) => assert_eq!(msg, "boom"),
            other => panic!("expected Remote error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dropping_guard_evicts_entry() {
        let pending = Arc::new(PendingRequests::new());
        let (_id, guard, _rx) = pending.register();
        assert_eq!(pending.len(), 1);

        drop(guard);
        assert!(
            pending.is_empty(),
            "dropping the guard (timeout path) must evict the entry"
        );
    }

    #[tokio::test]
    async fn completing_unknown_id_is_noop() {
        let pending = Arc::new(PendingRequests::new());
        // No panic, no effect.
        pending.complete(12345, Ok(Vec::new()));
        assert!(pending.is_empty());
    }
}
