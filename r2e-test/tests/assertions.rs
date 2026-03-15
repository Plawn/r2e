use r2e_test::json_contains;
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
