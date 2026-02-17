use r2e_test::resolve_path;
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
