use r2e_grpc::registry::GrpcServiceRegistry;

#[test]
fn new_registry_is_empty() {
    let registry = GrpcServiceRegistry::new();
    assert!(registry.take().is_none());
    assert!(registry.service_names().is_empty());
}

#[test]
fn add_and_take() {
    let registry = GrpcServiceRegistry::new();
    registry.add_service("pkg.ServiceA", |routes| routes);
    registry.add_service("pkg.ServiceB", |routes| routes);

    assert_eq!(
        registry.service_names(),
        vec!["pkg.ServiceA", "pkg.ServiceB"]
    );

    let (_routes, names) = registry.take().expect("two services were registered");
    assert_eq!(names, vec!["pkg.ServiceA", "pkg.ServiceB"]);

    // After take, the registry is empty.
    assert!(registry.take().is_none());
    assert!(registry.service_names().is_empty());
}

#[test]
fn clone_shares_state() {
    let registry = GrpcServiceRegistry::new();
    let cloned = registry.clone();

    registry.add_service("pkg.One", |routes| routes);
    cloned.add_service("pkg.Two", |routes| routes);

    let (_routes, names) = registry.take().expect("both clones fed the registry");
    assert_eq!(names, vec!["pkg.One", "pkg.Two"]);
}

#[test]
fn default_is_empty() {
    let registry = GrpcServiceRegistry::default();
    assert!(registry.take().is_none());
}
