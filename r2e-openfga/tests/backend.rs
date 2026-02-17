use r2e_openfga::MockBackend;

#[test]
fn test_mock_backend_check() {
    let backend = MockBackend::new();
    backend.add_tuple("user:alice", "viewer", "document:1");

    assert!(backend.has_tuple("user:alice", "viewer", "document:1"));
    assert!(!backend.has_tuple("user:bob", "viewer", "document:1"));
    assert!(!backend.has_tuple("user:alice", "editor", "document:1"));
}

#[test]
fn test_mock_backend_list_objects() {
    let backend = MockBackend::new();
    backend.add_tuple("user:alice", "viewer", "document:1");
    backend.add_tuple("user:alice", "viewer", "document:2");
    backend.add_tuple("user:alice", "editor", "document:3");

    let objects = backend.list_objects("user:alice", "viewer", "document");
    assert_eq!(objects.len(), 2);
    assert!(objects.contains(&"document:1".to_string()));
    assert!(objects.contains(&"document:2".to_string()));
}

#[test]
fn test_mock_backend_write_delete() {
    let backend = MockBackend::new();

    assert!(!backend.has_tuple("user:alice", "viewer", "document:1"));

    backend.add_tuple("user:alice", "viewer", "document:1");
    assert!(backend.has_tuple("user:alice", "viewer", "document:1"));

    backend.remove_tuple("user:alice", "viewer", "document:1");
    assert!(!backend.has_tuple("user:alice", "viewer", "document:1"));
}
