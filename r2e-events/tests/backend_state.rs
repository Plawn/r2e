//! Tests for `backend::state` — outcome-aware poller dispatch (P1.1).

use std::any::TypeId;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use r2e_events::backend::{BackendState, DeserializerFn, DispatchOutcome, Handler, TopicRegistry};
use r2e_events::{DlqPublisher, EventMetadata, HandlerResult, RetryPolicy};

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
async fn tracked_dispatch_skips_locally_dispatched_event() {
    let state = new_state();
    let calls = Arc::new(AtomicUsize::new(0));
    let handler = counting_handler(calls.clone(), || HandlerResult::Nack("never acked".into()));
    state
        .register_handler_with_deserializer::<TestEvent>(handler, test_deserializer())
        .await;

    let metadata = EventMetadata::new();
    state
        .locally_dispatched
        .lock()
        .unwrap()
        .insert(metadata.event_id);

    let completion = state
        .dispatch_from_poller_tracked(TypeId::of::<TestEvent>(), b"{}", metadata)
        .await;

    // Already handled by emit_and_wait locally: ack the broker copy, skip handlers.
    assert_eq!(completion.outcome().await, DispatchOutcome::Ack);
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
