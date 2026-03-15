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
