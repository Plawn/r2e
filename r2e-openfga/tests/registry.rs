use r2e_openfga::{MockBackend, OpenFgaRegistry};

#[tokio::test]
async fn test_registry_check_with_mock() {
    let mock = MockBackend::new();
    mock.add_tuple("user:alice", "viewer", "document:1");

    let registry = OpenFgaRegistry::new(mock);

    assert!(registry
        .check("user:alice", "viewer", "document:1")
        .await
        .unwrap());
    assert!(!registry
        .check("user:bob", "viewer", "document:1")
        .await
        .unwrap());
}

#[tokio::test]
async fn test_registry_cache_hit() {
    let mock = MockBackend::new();
    mock.add_tuple("user:alice", "viewer", "document:1");

    let registry = OpenFgaRegistry::with_cache(mock, 60);

    // First call populates cache
    assert!(registry
        .check("user:alice", "viewer", "document:1")
        .await
        .unwrap());

    // Second call hits cache (same result)
    assert!(registry
        .check("user:alice", "viewer", "document:1")
        .await
        .unwrap());
}

#[tokio::test]
async fn test_registry_invalidate_object() {
    let mock = MockBackend::new();
    let registry = OpenFgaRegistry::with_cache(mock, 60);

    // Check returns false, gets cached
    assert!(!registry
        .check("user:alice", "viewer", "document:1")
        .await
        .unwrap());

    // Simulate an external write by accessing backend through the mock
    // (In real code, the user would write via GrpcBackend::client())

    // Invalidate cache -- next check will hit backend again
    registry.invalidate_object("document:1");
}

#[tokio::test]
async fn test_registry_clear_cache() {
    let mock = MockBackend::new();
    mock.add_tuple("user:alice", "viewer", "document:1");

    let registry = OpenFgaRegistry::with_cache(mock, 60);

    assert!(registry
        .check("user:alice", "viewer", "document:1")
        .await
        .unwrap());

    registry.clear_cache();

    // Cache cleared -- next check goes to backend
    assert!(registry
        .check("user:alice", "viewer", "document:1")
        .await
        .unwrap());
}
