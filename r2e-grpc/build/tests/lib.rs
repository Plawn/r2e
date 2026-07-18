use r2e_grpc_build::{render_aggregator, PackageTree};

#[test]
fn empty_tree_renders_stub_descriptor() {
    let out = render_aggregator(&PackageTree::default(), false);
    assert!(out.contains("pub const FILE_DESCRIPTOR_SET: &[u8] = &[];"));
    assert!(!out.contains("include!"));
}

#[test]
fn single_package_renders_one_module() {
    let mut tree = PackageTree::default();
    tree.insert("greeter");
    let out = render_aggregator(&tree, true);
    assert!(out
        .contains("pub const FILE_DESCRIPTOR_SET: &[u8] = include_bytes!(\"r2e_descriptor.bin\");"));
    assert!(out.contains("pub mod greeter {"));
    assert!(out.contains("include!(\"greeter.rs\");"));
}

#[test]
fn dotted_packages_nest_and_merge() {
    let mut tree = PackageTree::default();
    tree.insert("foo.v1");
    tree.insert("foo.v2");
    tree.insert("foo");
    let out = render_aggregator(&tree, true);
    // One `foo` module containing its own include plus both versions.
    assert_eq!(out.matches("pub mod foo {").count(), 1);
    assert!(out.contains("include!(\"foo.rs\");"));
    assert!(out.contains("pub mod v1 {"));
    assert!(out.contains("include!(\"foo.v1.rs\");"));
    assert!(out.contains("pub mod v2 {"));
    assert!(out.contains("include!(\"foo.v2.rs\");"));
}

#[test]
fn empty_package_includes_at_root() {
    let mut tree = PackageTree::default();
    tree.insert("");
    let out = render_aggregator(&tree, true);
    assert!(out.contains("include!(\"_.rs\");"));
    assert!(!out.contains("pub mod"));
}

#[test]
fn keyword_segments_are_escaped() {
    let mut tree = PackageTree::default();
    tree.insert("type.self");
    let out = render_aggregator(&tree, true);
    assert!(out.contains("pub mod r#type {"));
    assert!(out.contains("pub mod self_ {"));
    // The include path keeps the original package name.
    assert!(out.contains("include!(\"type.self.rs\");"));
}

#[test]
fn output_is_deterministic_and_sorted() {
    let mut tree = PackageTree::default();
    tree.insert("zeta");
    tree.insert("alpha");
    let out = render_aggregator(&tree, true);
    let alpha = out.find("pub mod alpha").unwrap();
    let zeta = out.find("pub mod zeta").unwrap();
    assert!(alpha < zeta);
}
