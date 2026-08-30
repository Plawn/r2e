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
    registry
        .add_service("pkg.ServiceA", None, |routes| routes)
        .expect("first registration of pkg.ServiceA");
    registry
        .add_service("pkg.ServiceB", None, |routes| routes)
        .expect("first registration of pkg.ServiceB");

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

    registry
        .add_service("pkg.One", None, |routes| routes)
        .expect("first registration of pkg.One");
    cloned
        .add_service("pkg.Two", None, |routes| routes)
        .expect("first registration of pkg.Two");

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
    registry
        .add_service("pkg.ServiceA", Some(SET_A), |routes| routes)
        .expect("first registration of pkg.ServiceA");
    // Same proto compilation: the identical set is stored once.
    registry
        .add_service("pkg.ServiceB", Some(SET_A), |routes| routes)
        .expect("first registration of pkg.ServiceB");
    registry
        .add_service("pkg.ServiceC", Some(SET_B), |routes| routes)
        .expect("first registration of pkg.ServiceC");
    registry
        .add_service("pkg.ServiceD", None, |routes| routes)
        .expect("first registration of pkg.ServiceD");

    let services = registry.take().expect("four services were registered");
    assert_eq!(
        services.names,
        vec![
            "pkg.ServiceA",
            "pkg.ServiceB",
            "pkg.ServiceC",
            "pkg.ServiceD"
        ]
    );
    assert_eq!(services.descriptors, vec![SET_A, SET_B]);
}

#[test]
fn a_duplicate_service_name_is_rejected_without_building_the_service() {
    let registry = GrpcServiceRegistry::new();
    let mut builds = 0;

    registry
        .add_service("pkg.Service", None, |routes| {
            builds += 1;
            routes
        })
        .expect("first registration");

    let err = registry
        .add_service("pkg.Service", None, |routes| {
            builds += 1;
            routes
        })
        .expect_err("the same service name must not be registered twice");

    assert_eq!(err.name, "pkg.Service");
    assert!(
        err.to_string().contains("pkg.Service"),
        "the error must name the clashing service: {err}"
    );
    // The rejected registration never ran `add`: the service is not even built,
    // so tonic never sees two overlapping route sets for one name.
    assert_eq!(builds, 1, "the duplicate must not be built");
    assert_eq!(registry.service_names(), vec!["pkg.Service"]);
}
