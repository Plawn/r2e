//! `apply_reflection`: descriptor plumbing into the v1 + v1alpha reflection
//! services (non-regression for task #1037 — the two builders must never
//! share state; each build decodes every registered descriptor set).
#![cfg(feature = "reflection")]

use r2e_grpc::server::apply_reflection;
use r2e_grpc::RegisteredServices;

/// A valid encoded `FileDescriptorSet` without needing protoc in the test:
/// the reflection service's own descriptor, shipped by tonic-reflection.
const DESCRIPTOR: &[u8] = tonic_reflection::pb::v1::FILE_DESCRIPTOR_SET;

fn services() -> RegisteredServices {
    RegisteredServices {
        routes: tonic::service::Routes::default(),
        names: vec!["catalog.CatalogService"],
        descriptors: Vec::new(),
    }
}

#[test]
fn builds_both_reflection_services_from_a_registered_descriptor() {
    // Descriptor collected from `#[grpc_routes(..., descriptor = ...)]`,
    // reflection enabled with plain `with_reflection()` (empty extra list).
    let mut services = services();
    services.descriptors.push(DESCRIPTOR);

    // Both the v1 and the v1alpha builder decode the descriptor set; a panic
    // or crash here is the #1037 regression.
    let services = apply_reflection(services, &Some(Vec::new()));

    assert_eq!(
        services.names,
        vec![
            "catalog.CatalogService",
            "grpc.reflection.v1.ServerReflection",
            "grpc.reflection.v1alpha.ServerReflection",
        ]
    );
    assert_eq!(services.descriptors, vec![DESCRIPTOR]);
}

#[test]
fn merges_extra_descriptors_and_deduplicates() {
    // Same descriptor arriving both from a service and from
    // `with_reflection_descriptor`: registered once.
    let mut services = services();
    services.descriptors.push(DESCRIPTOR);

    let services = apply_reflection(services, &Some(vec![DESCRIPTOR]));

    assert_eq!(services.descriptors, vec![DESCRIPTOR]);
    assert_eq!(services.names.len(), 3);
}

#[test]
fn reflection_disabled_leaves_services_untouched() {
    let services = apply_reflection(services(), &None);
    assert_eq!(services.names, vec!["catalog.CatalogService"]);
    assert!(services.descriptors.is_empty());
}

#[test]
fn reflection_without_any_descriptor_still_builds() {
    // Warned at runtime, but must not fail: the reflection services expose
    // only themselves.
    let services = apply_reflection(services(), &Some(Vec::new()));
    assert_eq!(services.names.len(), 3);
}
