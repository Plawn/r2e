use bytes::Bytes;
use http::header::HeaderMap;
use http::StatusCode;
use r2e_test::{json_contains, TestResponse};
use serde_json::json;

// ─── json_contains tests ───

#[test]
fn test_json_contains_exact_match() {
    let actual = json!({"name": "Alice", "age": 30});
    let expected = json!({"name": "Alice", "age": 30});
    assert!(json_contains(&actual, &expected));
}

#[test]
fn test_json_contains_subset_match() {
    let actual = json!({"name": "Alice", "age": 30, "city": "NYC"});
    let expected = json!({"name": "Alice"});
    assert!(json_contains(&actual, &expected));
}

#[test]
fn test_json_contains_nested_subset() {
    let actual = json!({"user": {"name": "Alice", "age": 30}});
    let expected = json!({"user": {"name": "Alice"}});
    assert!(json_contains(&actual, &expected));
}

#[test]
fn test_json_contains_array_subset() {
    let actual = json!({"tags": ["rust", "web", "api"]});
    let expected = json!({"tags": ["rust", "api"]});
    assert!(json_contains(&actual, &expected));
}

#[test]
fn test_json_contains_mismatch() {
    let actual = json!({"name": "Alice"});
    let expected = json!({"name": "Bob"});
    assert!(!json_contains(&actual, &expected));
}

#[test]
fn test_json_contains_missing_key() {
    let actual = json!({"name": "Alice"});
    let expected = json!({"email": "alice@example.com"});
    assert!(!json_contains(&actual, &expected));
}

#[test]
fn test_json_contains_scalar() {
    assert!(json_contains(&json!(42), &json!(42)));
    assert!(!json_contains(&json!(42), &json!(43)));
    assert!(json_contains(&json!("hello"), &json!("hello")));
}

#[test]
fn test_json_contains_empty_expected() {
    let actual = json!({"name": "Alice"});
    let expected = json!({});
    assert!(json_contains(&actual, &expected));
}

#[test]
fn test_json_contains_array_element_objects() {
    let actual = json!([
        {"id": 1, "name": "Alice"},
        {"id": 2, "name": "Bob"}
    ]);
    let expected = json!([{"name": "Bob"}]);
    assert!(json_contains(&actual, &expected));
}

// ─── json_shape (tested indirectly via the public json_contains and type checks) ───
// json_shape_errors is private, but assert_json_shape is tested via TestResponse.
// Here we test the json_contains logic thoroughly which is the public API.

#[test]
fn test_json_contains_deeply_nested() {
    let actual = json!({
        "response": {
            "data": {
                "users": [
                    {"id": 1, "profile": {"bio": "hello", "verified": true}},
                    {"id": 2, "profile": {"bio": "world", "verified": false}},
                ]
            }
        }
    });
    let expected = json!({
        "response": {
            "data": {
                "users": [{"profile": {"verified": true}}]
            }
        }
    });
    assert!(json_contains(&actual, &expected));
}

#[test]
fn test_json_contains_null_handling() {
    let actual = json!({"key": null});
    let expected = json!({"key": null});
    assert!(json_contains(&actual, &expected));
}

// ─── assert_json_shape: each check must FAIL on wrong input ───

fn json_response(body: serde_json::Value) -> TestResponse {
    TestResponse::from_parts(
        StatusCode::OK,
        HeaderMap::new(),
        Bytes::from(serde_json::to_vec(&body).unwrap()),
    )
}

/// The panic message of `f`, which must panic.
fn panic_message(f: impl FnOnce() + std::panic::UnwindSafe) -> String {
    let err = std::panic::catch_unwind(f).expect_err("the assertion should have panicked");
    err.downcast_ref::<String>()
        .cloned()
        .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default()
}

#[test]
fn test_json_shape_matching_body_passes() {
    json_response(json!({
        "id": 1,
        "name": "Alice",
        "active": true,
        "note": null,
        "tags": ["a", "b"],
        "profile": {"age": 30},
    }))
    .assert_json_shape(json!({
        "id": 0,
        "name": "",
        "active": false,
        "note": null,
        "tags": [""],
        "profile": {"age": 0},
    }));
}

#[test]
fn test_json_shape_scalar_type_mismatch_fails_with_types() {
    let msg = panic_message(|| {
        json_response(json!({"id": "one"})).assert_json_shape(json!({"id": 0}));
    });
    assert!(
        msg.contains("at \"id\": expected number, got string"),
        "unexpected message: {msg}"
    );
}

#[test]
fn test_json_shape_root_type_names() {
    for (actual, schema, expected, got) in [
        (json!(null), json!(true), "boolean", "null"),
        (json!(true), json!(""), "string", "boolean"),
        (json!(1), json!([]), "array", "number"),
        (json!("s"), json!({}), "object", "string"),
        (json!([1]), json!(null), "null", "array"),
        (json!({}), json!(0), "number", "object"),
    ] {
        let msg = panic_message(move || {
            json_response(actual).assert_json_shape(schema);
        });
        let line = format!("at \"<root>\": expected {expected}, got {got}");
        assert!(msg.contains(&line), "expected `{line}` in: {msg}");
    }
}

#[test]
fn test_json_shape_missing_key_fails() {
    let msg = panic_message(|| {
        json_response(json!({"user": {"name": "Alice"}}))
            .assert_json_shape(json!({"user": {"name": "", "email": ""}}));
    });
    assert!(
        msg.contains("missing key \"user.email\""),
        "unexpected message: {msg}"
    );
}

#[test]
fn test_json_shape_array_item_mismatch_fails() {
    let msg = panic_message(|| {
        json_response(json!({"tags": ["a", 2]})).assert_json_shape(json!({"tags": [""]}));
    });
    assert!(
        msg.contains("at \"tags[1]\": expected string, got number"),
        "unexpected message: {msg}"
    );
}

#[test]
fn test_json_shape_empty_schema_array_accepts_any_items() {
    json_response(json!({"tags": ["a", 2, null]})).assert_json_shape(json!({"tags": []}));
}
