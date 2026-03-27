use bytes::Bytes;
use http::header::HeaderMap;
use http::StatusCode;
use r2e_test::{ParsedSseEvent, TestResponse};

fn sse_response(body: &str) -> TestResponse {
    let mut headers = HeaderMap::new();
    headers.insert("content-type", "text/event-stream".parse().unwrap());
    TestResponse::from_parts(StatusCode::OK, headers, Bytes::from(body.to_string()))
}

#[test]
fn test_parse_data_only_events() {
    let resp = sse_response("data: hello\n\ndata: world\n\n");
    let events = resp.sse_events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0], ParsedSseEvent { event: None, data: "hello".into() });
    assert_eq!(events[1], ParsedSseEvent { event: None, data: "world".into() });
}

#[test]
fn test_parse_typed_events() {
    let resp = sse_response("event: update\ndata: {\"n\":1}\n\n");
    let events = resp.sse_events();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0],
        ParsedSseEvent {
            event: Some("update".into()),
            data: "{\"n\":1}".into(),
        }
    );
}

#[test]
fn test_parse_multi_data_lines() {
    let resp = sse_response("data: line1\ndata: line2\n\n");
    let events = resp.sse_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data, "line1\nline2");
}

#[test]
fn test_assert_sse_event_passes() {
    let resp = sse_response("event: ping\ndata: pong\n\n");
    resp.assert_sse_event("ping", "pong");
}

#[test]
fn test_assert_sse_data_passes() {
    let resp = sse_response("data: hello\n\n");
    resp.assert_sse_data("hello");
}

#[test]
fn test_mixed_events() {
    let body = "data: plain\n\nevent: typed\ndata: value\n\ndata: another\n\n";
    let resp = sse_response(body);
    let events = resp.sse_events();
    assert_eq!(events.len(), 3);
    resp.assert_sse_data("plain")
        .assert_sse_event("typed", "value")
        .assert_sse_data("another");
}

#[test]
fn test_empty_body_produces_no_events() {
    let resp = sse_response("");
    assert!(resp.sse_events().is_empty());
}
