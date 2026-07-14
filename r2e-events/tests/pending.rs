//! Tests for the shared request-reply plumbing: the pending-request
//! correlation map, reply-header codec, request/reply topic conventions, and
//! the shared `await_reply` request tail.

use std::sync::Arc;
use std::time::Duration;

use r2e_events::backend::{
    await_reply, decode_metadata, decode_reply_headers, encode_metadata, encode_reply_headers,
    instance_id, reply_topic, request_topic, responder_group, PendingRequests, ReplyHeaders,
    HEADER_CORRELATION_ID, HEADER_REPLY_ERROR, HEADER_REPLY_TO, HEADER_REQUEST_ID,
};
use r2e_events::{EventBusError, EventMetadata};
use serde::{Deserialize, Serialize};

#[test]
fn responder_group_depends_only_on_request_topic() {
    let topic = request_topic("orders");
    assert_eq!(responder_group(&topic), "r2e.responders.orders.requests");
}

#[tokio::test(flavor = "multi_thread")]
async fn register_then_complete_delivers_reply() {
    let pending = Arc::new(PendingRequests::new());
    let (id, _guard, rx) = pending.register();
    assert_eq!(pending.len(), 1);

    pending.complete(id, Ok(b"reply".to_vec()));
    let received = rx.await.unwrap();
    assert_eq!(received.unwrap(), b"reply".to_vec());
    // complete removed the entry.
    assert!(pending.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn register_yields_unique_ids() {
    let pending = Arc::new(PendingRequests::new());
    let (id1, _g1, _rx1) = pending.register();
    let (id2, _g2, _rx2) = pending.register();
    assert_ne!(id1, id2);
    assert_eq!(pending.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_guard_removes_the_entry() {
    let pending = Arc::new(PendingRequests::new());
    let (id, guard, _rx) = pending.register();
    assert_eq!(pending.len(), 1);

    drop(guard);
    assert!(pending.is_empty());

    // A late completion after the guard dropped is a harmless no-op.
    pending.complete(id, Ok(Vec::new()));
    assert!(pending.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn complete_with_remote_error_propagates() {
    let pending = Arc::new(PendingRequests::new());
    let (id, _guard, rx) = pending.register();

    pending.complete(id, Err(EventBusError::Remote("boom".to_string())));
    let received = rx.await.unwrap();
    assert!(matches!(received, Err(EventBusError::Remote(msg)) if msg == "boom"));
}

// ── complete_reply ─────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn complete_reply_ok_delivers_payload() {
    let pending = Arc::new(PendingRequests::new());
    let (id, _guard, rx) = pending.register();

    let headers = ReplyHeaders { request_id: id, reply_to: None, reply_error: None };
    pending.complete_reply(&headers, b"payload".to_vec());

    let received = rx.await.unwrap();
    assert_eq!(received.unwrap(), b"payload".to_vec());
    assert!(pending.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn complete_reply_error_maps_to_remote() {
    let pending = Arc::new(PendingRequests::new());
    let (id, _guard, rx) = pending.register();

    let headers = ReplyHeaders {
        request_id: id,
        reply_to: None,
        reply_error: Some("handler failed".to_string()),
    };
    // Payload is ignored when an error is present.
    pending.complete_reply(&headers, b"null".to_vec());

    let received = rx.await.unwrap();
    assert!(matches!(received, Err(EventBusError::Remote(msg)) if msg == "handler failed"));
}

#[tokio::test(flavor = "multi_thread")]
async fn complete_reply_unknown_id_is_noop() {
    let pending = Arc::new(PendingRequests::new());
    let (_id, guard, _rx) = pending.register();
    drop(guard);

    let headers = ReplyHeaders { request_id: 999, reply_to: None, reply_error: None };
    pending.complete_reply(&headers, b"x".to_vec());
    assert!(pending.is_empty());
}

// ── await_reply ────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct Pong {
    value: u32,
}

#[tokio::test(flavor = "multi_thread")]
async fn await_reply_deserializes_success() {
    let pending = Arc::new(PendingRequests::new());
    let (id, guard, rx) = pending.register();

    let bytes = serde_json::to_vec(&Pong { value: 7 }).unwrap();
    pending.complete(id, Ok(bytes));

    let resp: Pong = await_reply(rx, Duration::from_secs(5), std::future::pending())
        .await
        .expect("reply delivered");
    assert_eq!(resp, Pong { value: 7 });
    drop(guard);
}

#[tokio::test(flavor = "multi_thread")]
async fn await_reply_times_out() {
    let pending = Arc::new(PendingRequests::new());
    let (_id, guard, rx) = pending.register();

    // Never completed → the short timeout fires.
    let result: Result<Pong, _> =
        await_reply(rx, Duration::from_millis(20), std::future::pending()).await;
    assert!(matches!(result, Err(EventBusError::RequestTimeout)));
    drop(guard);
}

#[tokio::test(flavor = "multi_thread")]
async fn await_reply_dropped_sender_is_timeout() {
    let pending = Arc::new(PendingRequests::new());
    let (_id, guard, rx) = pending.register();

    // Dropping the guard removes the map entry, dropping the reply Sender.
    drop(guard);

    let result: Result<Pong, _> =
        await_reply(rx, Duration::from_secs(5), std::future::pending()).await;
    assert!(matches!(result, Err(EventBusError::RequestTimeout)));
}

#[tokio::test(flavor = "multi_thread")]
async fn await_reply_shutdown_wins() {
    let pending = Arc::new(PendingRequests::new());
    let (_id, guard, rx) = pending.register();

    // Shutdown future is already ready; the reply never arrives.
    let result: Result<Pong, _> =
        await_reply(rx, Duration::from_secs(5), std::future::ready(())).await;
    assert!(matches!(result, Err(EventBusError::Shutdown)));
    drop(guard);
}

#[tokio::test(flavor = "multi_thread")]
async fn await_reply_observes_pre_cancelled_shutdown_token() {
    let pending = Arc::new(PendingRequests::new());
    let (_id, guard, rx) = pending.register();
    let cancel = tokio_util::sync::CancellationToken::new();
    cancel.cancel();

    let result: Result<Pong, _> =
        await_reply(rx, Duration::from_secs(5), cancel.cancelled()).await;
    assert!(matches!(result, Err(EventBusError::Shutdown)));
    drop(guard);
}

#[tokio::test(flavor = "multi_thread")]
async fn await_reply_bad_payload_is_serialization_error() {
    let pending = Arc::new(PendingRequests::new());
    let (id, guard, rx) = pending.register();

    // Not valid JSON for `Pong`.
    pending.complete(id, Ok(b"not-json".to_vec()));

    let result: Result<Pong, _> =
        await_reply(rx, Duration::from_secs(5), std::future::pending()).await;
    assert!(matches!(result, Err(EventBusError::Serialization(_))));
    drop(guard);
}

// ── reply-header codec ─────────────────────────────────────────────────

#[test]
fn reply_headers_roundtrip_request() {
    let pairs: Vec<_> =
        encode_reply_headers(1234567890123456789, Some("app.replies.abcd"), None).collect();
    // The request id owns its own dedicated header slot.
    assert!(pairs.iter().any(|(k, _)| k == HEADER_REQUEST_ID));
    // It must NOT leak into the user's correlation-id slot.
    assert!(!pairs.iter().any(|(k, _)| k == HEADER_CORRELATION_ID));
    assert!(pairs.iter().any(|(k, v)| k == HEADER_REPLY_TO && v == "app.replies.abcd"));
    assert!(!pairs.iter().any(|(k, _)| k == HEADER_REPLY_ERROR));

    let decoded = decode_reply_headers(pairs.iter().map(|(k, v)| (k.as_ref(), v.as_str())))
        .expect("request id present");
    assert_eq!(decoded.request_id, 1234567890123456789);
    assert_eq!(decoded.reply_to.as_deref(), Some("app.replies.abcd"));
    assert_eq!(decoded.reply_error, None);
}

#[test]
fn reply_headers_roundtrip_error_reply() {
    let pairs: Vec<_> = encode_reply_headers(42, None, Some("handler failed")).collect();
    let decoded = decode_reply_headers(pairs.iter().map(|(k, v)| (k.as_ref(), v.as_str())))
        .expect("request id present");
    assert_eq!(decoded.request_id, 42);
    assert_eq!(decoded.reply_to, None);
    assert_eq!(decoded.reply_error.as_deref(), Some("handler failed"));
}

#[test]
fn decode_reply_headers_without_request_id_is_none() {
    let pairs: Vec<(String, String)> = vec![("unrelated".to_string(), "x".to_string())];
    assert!(decode_reply_headers(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str()))).is_none());
}

#[test]
fn user_correlation_id_survives_alongside_request_id() {
    // A request carries the user's own correlation_id (metadata) AND the
    // internal request id. The two must not collide: encode_metadata owns the
    // correlation-id slot, encode_reply_headers owns the request-id slot.
    let mut metadata = EventMetadata::new();
    metadata.correlation_id = Some("user-corr-123".to_string());

    let pairs: Vec<_> = encode_metadata(&metadata)
        .chain(encode_reply_headers(
            777,
            Some("app.replies.f00d"),
            None,
        ))
        .collect();

    // The user correlation id decodes unchanged.
    let decoded_meta = decode_metadata(pairs.iter().map(|(k, v)| (k.as_ref(), v.as_str())));
    assert_eq!(decoded_meta.correlation_id.as_deref(), Some("user-corr-123"));

    // The internal request id decodes independently.
    let decoded_reply = decode_reply_headers(pairs.iter().map(|(k, v)| (k.as_ref(), v.as_str())))
        .expect("request id present");
    assert_eq!(decoded_reply.request_id, 777);
    assert_eq!(decoded_reply.reply_to.as_deref(), Some("app.replies.f00d"));
}

// ── topic conventions ──────────────────────────────────────────────────

#[test]
fn request_topic_appends_suffix() {
    assert_eq!(request_topic("orders.created"), "orders.created.requests");
}

#[test]
fn reply_topic_embeds_instance_id() {
    let topic = reply_topic("app", 0xabcd);
    assert_eq!(topic, "app.replies.000000000000abcd");
}

#[test]
fn reply_topic_differs_per_instance() {
    // Two bus instances sharing a config get distinct reply topics.
    let a = reply_topic("app", instance_id());
    let b = reply_topic("app", instance_id());
    assert_ne!(a, b);
    assert!(a.starts_with("app.replies."));
    assert!(b.starts_with("app.replies."));
}

#[test]
fn instance_id_is_fresh_per_call() {
    assert_ne!(instance_id(), instance_id());
}
