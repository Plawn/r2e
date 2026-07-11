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
    registry.add_service("pkg.ServiceA", None, |routes| routes);
    registry.add_service("pkg.ServiceB", None, |routes| routes);

    assert_eq!(
        registry.service_names(),
        vec!["pkg.ServiceA", "pkg.ServiceB"]
    );

    let services = registry.take().expect("two services were registered");
    assert_eq!(services.names, vec!["pkg.ServiceA", "pkg.ServiceB"]);
    assert!(services.descriptors.is_empty());

    // After take, the registry is empty.
    assert!(registry.take().is_none());
    assert!(registry.service_names().is_empty());
}

#[test]
fn clone_shares_state() {
    let registry = GrpcServiceRegistry::new();
    let cloned = registry.clone();

    registry.add_service("pkg.One", None, |routes| routes);
    cloned.add_service("pkg.Two", None, |routes| routes);

    let services = registry.take().expect("both clones fed the registry");
    assert_eq!(services.names, vec!["pkg.One", "pkg.Two"]);
}

#[test]
fn default_is_empty() {
    let registry = GrpcServiceRegistry::default();
    assert!(registry.take().is_none());
}

#[test]
fn descriptors_are_collected_and_deduplicated() {
    static SET_A: &[u8] = b"descriptor-set-a";
    static SET_B: &[u8] = b"descriptor-set-b";

    let registry = GrpcServiceRegistry::new();
    registry.add_service("pkg.ServiceA", Some(SET_A), |routes| routes);
    // Same proto compilation: the identical set is stored once.
    registry.add_service("pkg.ServiceB", Some(SET_A), |routes| routes);
    registry.add_service("pkg.ServiceC", Some(SET_B), |routes| routes);
    registry.add_service("pkg.ServiceD", None, |routes| routes);

    let services = registry.take().expect("four services were registered");
    assert_eq!(
        services.names,
        vec!["pkg.ServiceA", "pkg.ServiceB", "pkg.ServiceC", "pkg.ServiceD"]
    );
    assert_eq!(services.descriptors, vec![SET_A, SET_B]);
}
