//! `GrpcMarker` — the provision `requires_plugins(GrpcServer)` checks for — is
//! **sealed**: it carries a private field, so no crate outside `r2e-grpc` can
//! construct one. Hand-providing it to fake the plugin (which would satisfy the
//! provision-based required-plugin check while leaving no registry to drain) is
//! a compile error, which is what makes "a module with `grpc_services(..)` and
//! no `GrpcServer` plugin does not compile" an exact claim.

use r2e::prelude::*;
use r2e::r2e_grpc::GrpcMarker;

#[module]
pub struct EmptyModule;

fn main() {
    // Unit-struct spoof: `GrpcMarker` is not a value.
    let _ = r2e::AppBuilder::new()
        .provide(GrpcMarker)
        .register_module::<EmptyModule>();
}

fn tuple_spoof() {
    // Tuple-struct spoof: the constructor is private to `r2e-grpc`.
    let _ = r2e::AppBuilder::new().provide(GrpcMarker(()));
}
