use r2e_grpc::registry::GrpcServiceRegistry;

#[test]
fn new_registry_is_empty() {
    let registry = GrpcServiceRegistry::new();
    let items = registry.take_all();
    assert!(items.is_empty());
}

#[test]
fn add_and_take_all() {
    let registry = GrpcServiceRegistry::new();
    registry.add(Box::new(42_i32));
    registry.add(Box::new("hello".to_string()));

    let items = registry.take_all();
    assert_eq!(items.len(), 2);

    // After take_all, registry is empty
    let items_after = registry.take_all();
    assert!(items_after.is_empty());
}

#[test]
fn clone_shares_state() {
    let registry = GrpcServiceRegistry::new();
    let cloned = registry.clone();

    registry.add(Box::new(1_i32));
    cloned.add(Box::new(2_i32));

    let items = registry.take_all();
    assert_eq!(items.len(), 2);
}

#[test]
fn downcast_works() {
    let registry = GrpcServiceRegistry::new();
    registry.add(Box::new(42_i32));

    let items = registry.take_all();
    let val = items.into_iter().next().unwrap();
    let downcasted = val.downcast::<i32>().unwrap();
    assert_eq!(*downcasted, 42);
}

#[test]
fn default_is_empty() {
    let registry = GrpcServiceRegistry::default();
    assert!(registry.take_all().is_empty());
}
