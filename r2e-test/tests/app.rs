use bytes::Bytes;
use http::header::{HeaderMap, HeaderValue};
use http::StatusCode;
use r2e_test::{resolve_path, TestApp, TestJwt, TestResponse};
use serde_json::{json, Value};

#[test]
fn test_resolve_simple_field() {
    let v = json!({"name": "Alice"});
    assert_eq!(resolve_path(&v, "name"), json!("Alice"));
}

#[test]
fn test_resolve_nested_field() {
    let v = json!({"user": {"name": "Bob"}});
    assert_eq!(resolve_path(&v, "user.name"), json!("Bob"));
}

#[test]
fn test_resolve_array_index() {
    let v = json!({"users": ["Alice", "Bob"]});
    assert_eq!(resolve_path(&v, "users[0]"), json!("Alice"));
    assert_eq!(resolve_path(&v, "users[1]"), json!("Bob"));
}

#[test]
fn test_resolve_array_nested() {
    let v = json!({"groups": [{"name": "admin", "tags": ["a", "b"]}]});
    assert_eq!(resolve_path(&v, "groups[0].name"), json!("admin"));
    assert_eq!(resolve_path(&v, "groups[0].tags[1]"), json!("b"));
}

#[test]
fn test_resolve_len() {
    let v = json!({"items": [1, 2, 3]});
    assert_eq!(resolve_path(&v, "items.len()"), json!(3));
}

#[test]
fn test_resolve_size() {
    let v = json!({"items": [1, 2]});
    assert_eq!(resolve_path(&v, "items.size()"), json!(2));
}

#[test]
fn test_resolve_nested_len() {
    let v = json!({"groups": [{"tags": ["a", "b", "c"]}]});
    assert_eq!(resolve_path(&v, "groups[0].tags.len()"), json!(3));
}

#[test]
fn test_resolve_missing_field() {
    let v = json!({"name": "Alice"});
    assert_eq!(resolve_path(&v, "missing"), Value::Null);
}

#[test]
fn test_resolve_object_len() {
    let v = json!({"meta": {"a": 1, "b": 2}});
    assert_eq!(resolve_path(&v, "meta.len()"), json!(2));
}

#[test]
fn test_resolve_chained_indices() {
    let v = json!({"m": [[1, 2], [3, 4]]});
    assert_eq!(resolve_path(&v, "m[1][0]"), json!(3));
    assert_eq!(resolve_path(&v, "m[0][1]"), json!(2));
    assert_eq!(resolve_path(&json!([[5, 6]]), "[0][1]"), json!(6));
}

#[test]
fn test_resolve_projects_fields_over_arrays() {
    let v = json!({"users": [{"name": "Alice"}, {"name": "Bob"}, {"id": 3}]});
    assert_eq!(
        resolve_path(&v, "users.name"),
        json!(["Alice", "Bob", null])
    );
    assert_eq!(resolve_path(&v, "users.name[1]"), json!("Bob"));
    assert_eq!(resolve_path(&v, "users.name.len()"), json!(3));
}

// ─── TestResponse accessors ───

fn response(content_type: Option<&str>, body: &'static str) -> TestResponse {
    let mut headers = HeaderMap::new();
    if let Some(ct) = content_type {
        headers.insert("content-type", HeaderValue::from_str(ct).unwrap());
    }
    headers.insert("x-request-id", HeaderValue::from_static("req-42"));
    TestResponse::from_parts(StatusCode::OK, headers, Bytes::from_static(body.as_bytes()))
}

#[test]
fn test_header_reads_value_case_insensitively() {
    let resp = response(None, "");
    assert_eq!(resp.header("x-request-id"), Some("req-42"));
    assert_eq!(resp.header("X-Request-Id"), Some("req-42"));
    assert_eq!(resp.header("x-missing"), None);
    assert_eq!(resp.header("not a header name"), None);
}

#[test]
fn test_content_type_and_is_json() {
    let json = response(Some("application/json; charset=utf-8"), "{}");
    assert_eq!(json.content_type(), Some("application/json; charset=utf-8"));
    assert!(json.is_json());

    let text = response(Some("text/plain"), "hi");
    assert_eq!(text.content_type(), Some("text/plain"));
    assert!(!text.is_json());

    let none = response(None, "{}");
    assert_eq!(none.content_type(), None);
    assert!(!none.is_json());
}

#[test]
fn test_bytes_returns_the_body() {
    let resp = response(None, "raw-body");
    assert_eq!(resp.bytes().as_ref(), b"raw-body");
}

#[test]
fn test_json_optional() {
    let resp = response(Some("application/json"), r#"{"n": 7}"#);
    assert_eq!(resp.json_optional::<Value>(), Some(json!({"n": 7})));
    assert_eq!(response(None, "not json").json_optional::<Value>(), None);
}

#[test]
fn test_assert_json_path_reads_the_parsed_body() {
    response(Some("application/json"), r#"{"user": {"name": "Alice"}}"#)
        .assert_json_path("user.name", "Alice");
}

#[test]
fn test_test_jwt_returns_the_attached_jwt() {
    let app = TestApp::new(r2e_core::http::Routes::new()).with_jwt(TestJwt::with_config(
        b"custom-secret",
        "custom-iss",
        "custom-aud",
    ));
    assert_eq!(app.test_jwt().issuer(), "custom-iss");
    assert_eq!(app.test_jwt().audience(), "custom-aud");
}
