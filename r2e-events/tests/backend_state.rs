//! Tests for `backend::state` — outcome-aware poller dispatch (P1.1).

use std::any::TypeId;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use r2e_events::backend::{BackendState, DeserializerFn, DispatchOutcome, Handler, TopicRegistry};
use r2e_events::{DlqPublisher, EventEnvelope, EventMetadata, HandlerResult, RetryPolicy};
use serde::{Deserialize, Serialize};

struct TestEvent;

fn test_deserializer() -> DeserializerFn {
    Arc::new(|_bytes: &[u8]| Ok(Arc::new(TestEvent) as Arc<dyn std::any::Any + Send + Sync>))
}

fn failing_deserializer() -> DeserializerFn {
    Arc::new(|_bytes: &[u8]| Err("bad payload".to_string()))
}

fn counting_handler(calls: Arc<AtomicUsize>, result: fn() -> HandlerResult) -> Handler {
    Arc::new(move |_event, _meta| {
        calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move { result() })
    })
}

fn new_state() -> Arc<BackendState> {
    Arc::new(BackendState::new(TopicRegistry::default()))
}

#[test]
fn default_topic_is_cached_by_type_id() {
    let state = new_state();

    let first = state.resolve_topic::<TestEvent>();
    let second = state.resolve_topic::<TestEvent>();

    assert_eq!(&*first, "backend_state.TestEvent");
    assert!(Arc::ptr_eq(&first, &second));
}

#[tokio::test(flavor = "multi_thread")]
async fn tracked_dispatch_all_ack() {
    let state = new_state();
    let calls = Arc::new(AtomicUsize::new(0));
    let handler = counting_handler(calls.clone(), || HandlerResult::Ack);
    state
        .register_handler_with_deserializer::<TestEvent>(handler, test_deserializer())
        .await;

    let completion = state
        .dispatch_from_poller_tracked(TypeId::of::<TestEvent>(), b"{}", EventMetadata::new())
        .await;

    assert_eq!(completion.outcome().await, DispatchOutcome::Ack);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn tracked_dispatch_nack_without_dlq_is_nack() {
    let state = new_state();
    let calls = Arc::new(AtomicUsize::new(0));
    let handler = counting_handler(calls.clone(), || HandlerResult::Nack("boom".into()));
    state
        .register_handler_with_deserializer::<TestEvent>(handler, test_deserializer())
        .await;

    let completion = state
        .dispatch_from_poller_tracked(TypeId::of::<TestEvent>(), b"{}", EventMetadata::new())
        .await;

    assert_eq!(completion.outcome().await, DispatchOutcome::Nack);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn tracked_dispatch_one_nack_among_acks_is_nack() {
    let state = new_state();
    let calls = Arc::new(AtomicUsize::new(0));
    state
        .register_handler_with_deserializer::<TestEvent>(
            counting_handler(calls.clone(), || HandlerResult::Ack),
            test_deserializer(),
        )
        .await;
    state
        .register_handler_with_deserializer::<TestEvent>(
            counting_handler(calls.clone(), || HandlerResult::Nack("boom".into())),
            test_deserializer(),
        )
        .await;

    let completion = state
        .dispatch_from_poller_tracked(TypeId::of::<TestEvent>(), b"{}", EventMetadata::new())
        .await;

    assert_eq!(completion.outcome().await, DispatchOutcome::Nack);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn tracked_dispatch_nack_with_dlq_capture_is_ack() {
    let published: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let published_clone = published.clone();
    let dlq: DlqPublisher = Arc::new(move |topic, _payload, _meta| {
        let published = published_clone.clone();
        Box::pin(async move {
            published.lock().unwrap().push(topic);
        })
    });
    let state = Arc::new(BackendState::with_dlq_publisher(
        TopicRegistry::default(),
        Some(dlq),
    ));

    let calls = Arc::new(AtomicUsize::new(0));
    let handler = counting_handler(calls.clone(), || HandlerResult::Nack("boom".into()));
    let policy = RetryPolicy {
        max_retries: 0,
        ..Default::default()
    }
    .with_dlq("dead-letters");
    state
        .register_handler_full::<serde_json::Value>(handler, Some(test_deserializer()), None, Some(policy))
        .await;

    let completion = state
        .dispatch_from_poller_tracked(TypeId::of::<serde_json::Value>(), b"{}", EventMetadata::new())
        .await;

    assert_eq!(completion.outcome().await, DispatchOutcome::Ack);
    assert_eq!(*published.lock().unwrap(), vec!["dead-letters".to_string()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn tracked_dispatch_no_handlers_is_ack() {
    let state = new_state();
    let completion = state
        .dispatch_from_poller_tracked(TypeId::of::<TestEvent>(), b"{}", EventMetadata::new())
        .await;
    assert_eq!(completion.outcome().await, DispatchOutcome::Ack);
}

#[tokio::test(flavor = "multi_thread")]
async fn tracked_dispatch_deserialize_failure_is_ack() {
    let state = new_state();
    let calls = Arc::new(AtomicUsize::new(0));
    let handler = counting_handler(calls.clone(), || HandlerResult::Ack);
    state
        .register_handler_with_deserializer::<TestEvent>(handler, failing_deserializer())
        .await;

    let completion = state
        .dispatch_from_poller_tracked(TypeId::of::<TestEvent>(), b"not json", EventMetadata::new())
        .await;

    // Poison messages are dropped (acked), not redelivered forever.
    assert_eq!(completion.outcome().await, DispatchOutcome::Ack);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn poison_message_routes_to_matching_dlqs_and_acks() {
    let published: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let published_clone = published.clone();
    let dlq: DlqPublisher = Arc::new(move |topic, _payload, _meta| {
        let published = published_clone.clone();
        Box::pin(async move {
            published.lock().unwrap().push(topic);
        })
    });
    let state = Arc::new(BackendState::with_dlq_publisher(
        TopicRegistry::default(),
        Some(dlq),
    ));

    let calls = Arc::new(AtomicUsize::new(0));
    let handler = counting_handler(calls.clone(), || HandlerResult::Ack);
    let policy = RetryPolicy {
        max_retries: 0,
        ..Default::default()
    }
    .with_dlq("poison-dlq");
    state
        .register_handler_full::<serde_json::Value>(
            handler,
            Some(failing_deserializer()),
            None,
            Some(policy),
        )
        .await;

    let completion = state
        .dispatch_from_poller_tracked(TypeId::of::<serde_json::Value>(), b"garbage", EventMetadata::new())
        .await;

    // Undecodable payload: parked in the configured DLQ, then acked away.
    assert_eq!(completion.outcome().await, DispatchOutcome::Ack);
    assert_eq!(*published.lock().unwrap(), vec!["poison-dlq".to_string()]);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn tracked_dispatch_panicking_handler_is_nack() {
    let state = new_state();
    let handler: Handler = Arc::new(|_event, _meta| Box::pin(async { panic!("handler blew up") }));
    state
        .register_handler_with_deserializer::<TestEvent>(handler, test_deserializer())
        .await;

    let completion = state
        .dispatch_from_poller_tracked(TypeId::of::<TestEvent>(), b"{}", EventMetadata::new())
        .await;

    assert_eq!(completion.outcome().await, DispatchOutcome::Nack);
}

#[tokio::test(flavor = "multi_thread")]
async fn untracked_dispatch_still_runs_handlers() {
    let state = new_state();
    let calls = Arc::new(AtomicUsize::new(0));
    let handler = counting_handler(calls.clone(), || HandlerResult::Ack);
    state
        .register_handler_with_deserializer::<TestEvent>(handler, test_deserializer())
        .await;

    state
        .dispatch_from_poller(TypeId::of::<TestEvent>(), b"{}", EventMetadata::new())
        .await;

    // Fire-and-forget: wait for the in-flight handler to finish.
    state
        .wait_in_flight(std::time::Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

// --- Request-reply responder registry (BackendState) ---

#[derive(Serialize, Deserialize)]
struct Ping {
    n: i64,
}

#[derive(Serialize, Deserialize)]
struct Pong {
    n: i64,
}

#[tokio::test(flavor = "multi_thread")]
async fn responder_invoke_returns_serialized_reply() {
    let state = new_state();
    state
        .register_responder::<Ping, Pong, String, _, _>(|env: EventEnvelope<Ping>| async move {
            Ok::<Pong, String>(Pong { n: env.event.n + 1 })
        })
        .await
        .unwrap();

    let payload = serde_json::to_vec(&Ping { n: 41 }).unwrap();
    let reply = state
        .invoke_responder(TypeId::of::<Ping>(), &payload, EventMetadata::new())
        .await
        .expect("responder registered")
        .expect("responder succeeded");
    let pong: Pong = serde_json::from_slice(&reply).unwrap();
    assert_eq!(pong.n, 42);
}

#[tokio::test(flavor = "multi_thread")]
async fn responder_error_surfaces_as_err_bytes() {
    let state = new_state();
    state
        .register_responder::<Ping, Pong, String, _, _>(|_env: EventEnvelope<Ping>| async move {
            Err::<Pong, String>("nope".to_string())
        })
        .await
        .unwrap();

    let payload = serde_json::to_vec(&Ping { n: 1 }).unwrap();
    let result = state
        .invoke_responder(TypeId::of::<Ping>(), &payload, EventMetadata::new())
        .await
        .expect("responder registered");
    assert_eq!(result, Err("nope".to_string()));
}

#[tokio::test(flavor = "multi_thread")]
async fn invoke_without_responder_is_none() {
    let state = new_state();
    let result = state
        .invoke_responder(TypeId::of::<Ping>(), b"{}", EventMetadata::new())
        .await;
    assert!(result.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn second_responder_for_same_type_is_rejected() {
    let state = new_state();
    state
        .register_responder::<Ping, Pong, String, _, _>(|_e: EventEnvelope<Ping>| async move {
            Ok::<Pong, String>(Pong { n: 0 })
        })
        .await
        .unwrap();
    let second = state
        .register_responder::<Ping, Pong, String, _, _>(|_e: EventEnvelope<Ping>| async move {
            Ok::<Pong, String>(Pong { n: 1 })
        })
        .await;
    assert!(second.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn unregister_responder_allows_reregistration() {
    let state = new_state();
    state
        .register_responder::<Ping, Pong, String, _, _>(|_e: EventEnvelope<Ping>| async move {
            Ok::<Pong, String>(Pong { n: 0 })
        })
        .await
        .unwrap();
    state.unregister_responder(TypeId::of::<Ping>()).await;
    state
        .register_responder::<Ping, Pong, String, _, _>(|e: EventEnvelope<Ping>| async move {
            Ok::<Pong, String>(Pong { n: e.event.n * 2 })
        })
        .await
        .expect("re-registration after unregister should succeed");

    let payload = serde_json::to_vec(&Ping { n: 21 }).unwrap();
    let reply = state
        .invoke_responder(TypeId::of::<Ping>(), &payload, EventMetadata::new())
        .await
        .unwrap()
        .unwrap();
    let pong: Pong = serde_json::from_slice(&reply).unwrap();
    assert_eq!(pong.n, 42);
}

// --- build_reply: single-sourced outcome mapping ---

#[tokio::test(flavor = "multi_thread")]
async fn build_reply_success_has_no_error() {
    let state = new_state();
    state
        .register_responder::<Ping, Pong, String, _, _>(|env: EventEnvelope<Ping>| async move {
            Ok::<Pong, String>(Pong { n: env.event.n + 1 })
        })
        .await
        .unwrap();

    let payload = serde_json::to_vec(&Ping { n: 41 }).unwrap();
    let (bytes, error) = state
        .build_reply(TypeId::of::<Ping>(), &payload, EventMetadata::new())
        .await;
    assert!(error.is_none());
    let pong: Pong = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(pong.n, 42);
}

#[tokio::test(flavor = "multi_thread")]
async fn build_reply_responder_error_carries_message_and_placeholder() {
    let state = new_state();
    state
        .register_responder::<Ping, Pong, String, _, _>(|_env: EventEnvelope<Ping>| async move {
            Err::<Pong, String>("nope".to_string())
        })
        .await
        .unwrap();

    let payload = serde_json::to_vec(&Ping { n: 1 }).unwrap();
    let (bytes, error) = state
        .build_reply(TypeId::of::<Ping>(), &payload, EventMetadata::new())
        .await;
    assert_eq!(error.as_deref(), Some("nope"));
    // Non-empty placeholder payload (some brokers reject empty payloads).
    assert_eq!(bytes, b"null".to_vec());
}

#[tokio::test(flavor = "multi_thread")]
async fn build_reply_without_responder_always_produces_error() {
    // A missing responder must produce an error reply, never a silent drop.
    let state = new_state();
    let (bytes, error) = state
        .build_reply(TypeId::of::<Ping>(), b"{}", EventMetadata::new())
        .await;
    assert_eq!(error.as_deref(), Some("no responder registered for request type"));
    assert_eq!(bytes, b"null".to_vec());
}
