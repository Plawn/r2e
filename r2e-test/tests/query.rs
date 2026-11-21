// These tests verify the query param builder and URI construction.
// Since build_uri is pub(crate), we test it indirectly through the public API
// by checking the constructed URI. We test the tokenization/path resolution
// as a proxy since the actual HTTP sending requires a router.

use r2e_test::resolve_path;
use serde_json::json;

#[test]
fn test_resolve_path_still_works_after_refactor() {
    let v = json!({"users": [{"name": "Alice"}, {"name": "Bob"}]});
    assert_eq!(resolve_path(&v, "users[0].name"), json!("Alice"));
    assert_eq!(resolve_path(&v, "users[1].name"), json!("Bob"));
    assert_eq!(resolve_path(&v, "users.len()"), json!(2));
}

#[test]
fn test_resolve_path_missing_returns_null() {
    let v = json!({"a": 1});
    assert_eq!(resolve_path(&v, "b"), serde_json::Value::Null);
    assert_eq!(resolve_path(&v, "a.b"), serde_json::Value::Null);
}

#[test]
fn test_resolve_path_deep_nesting() {
    let v = json!({"a": {"b": {"c": {"d": 42}}}});
    assert_eq!(resolve_path(&v, "a.b.c.d"), json!(42));
}

#[test]
fn test_resolve_path_by_reference_no_unnecessary_clone() {
    // Verify that resolve_path works with large nested structures
    let v = json!({
        "data": {
            "items": [
                {"id": 1, "tags": ["a", "b", "c"]},
                {"id": 2, "tags": ["d", "e"]},
            ]
        }
    });
    assert_eq!(resolve_path(&v, "data.items[0].tags[2]"), json!("c"));
    assert_eq!(resolve_path(&v, "data.items[1].tags.len()"), json!(2));
    assert_eq!(resolve_path(&v, "data.items.len()"), json!(2));
}

// ── Path parameters ──

use r2e_core::http::body::Body;
use r2e_core::http::Routes;
use r2e_test::TestApp;

async fn echo_uri(req: http::Request<Body>) -> String {
    req.uri().to_string()
}

/// An app answering every request with the URI it received.
fn echo_app() -> TestApp {
    TestApp::new(Routes::new().fallback(|_, req| echo_uri(req)))
}

#[r2e::test]
async fn path_param_fills_its_placeholder() {
    let resp = echo_app()
        .get("/users/{id}/posts/{slug}")
        .path_param("id", 42)
        .path_param("slug", "intro")
        .send()
        .await;
    assert_eq!(resp.text(), "/users/42/posts/intro");
}

#[r2e::test]
async fn path_param_is_encoded_as_one_segment() {
    let resp = echo_app()
        .get("/files/{name}")
        .path_param("name", "a b/c?d")
        .send()
        .await;
    assert_eq!(resp.text(), "/files/a%20b%2Fc%3Fd");
}

#[r2e::test]
async fn catch_all_path_param_keeps_its_separators() {
    let resp = echo_app()
        .get("/static/{*path}")
        .path_param("path", "css/main file.css")
        .send()
        .await;
    assert_eq!(resp.text(), "/static/css/main%20file.css");
}

#[r2e::test]
async fn path_param_composes_with_query_parameters() {
    let resp = echo_app()
        .get("/users/{id}?active=true")
        .path_param("id", 7)
        .query("page", "2")
        .send()
        .await;
    assert_eq!(resp.text(), "/users/7?active=true&page=2");
}

#[r2e::test]
async fn session_requests_take_path_params_too() {
    let app = echo_app();
    let resp = app
        .session()
        .get("/users/{id}")
        .path_param("id", "alice")
        .send()
        .await;
    assert_eq!(resp.text(), "/users/alice");
}

#[r2e::test]
#[should_panic(expected = "path parameter \"uid\" has no {uid} placeholder")]
async fn unknown_path_param_panics() {
    echo_app()
        .get("/users/{id}")
        .path_param("uid", 1)
        .send()
        .await;
}

#[r2e::test]
#[should_panic(expected = "path placeholder {slug} was not given a value")]
async fn unfilled_placeholder_panics() {
    echo_app()
        .get("/users/{id}/posts/{slug}")
        .path_param("id", 1)
        .send()
        .await;
}

// ── Projection over arrays ──

#[test]
fn field_read_on_an_array_projects_over_its_elements() {
    let v = json!({"users": [{"name": "Alice", "age": 30}, {"name": "Bob"}]});
    assert_eq!(resolve_path(&v, "users.name"), json!(["Alice", "Bob"]));
    // A missing key keeps the projection aligned with the source array.
    assert_eq!(resolve_path(&v, "users.age"), json!([30, null]));
    assert_eq!(resolve_path(&v, "users.name[1]"), json!("Bob"));
    assert_eq!(resolve_path(&v, "users.name.len()"), json!(2));
}

#[test]
fn projection_descends_through_nested_arrays() {
    let v = json!({"e": [{"a": {"b": 2}}, {"a": {"b": 3}}]});
    assert_eq!(resolve_path(&v, "e[0].a.b"), json!(2));
    assert_eq!(resolve_path(&v, "e.a.b"), json!([2, 3]));

    let nested = json!({"teams": [{"members": [{"id": 1}, {"id": 2}]}, {"members": [{"id": 3}]}]});
    assert_eq!(resolve_path(&nested, "teams.members.id"), json!([[1, 2], [3]]));
}

#[test]
fn root_array_is_addressed_by_a_leading_index() {
    let v = json!([{"name": "Alice"}]);
    assert_eq!(resolve_path(&v, "[0].name"), json!("Alice"));
    assert_eq!(resolve_path(&v, "name"), json!(["Alice"]));
}
