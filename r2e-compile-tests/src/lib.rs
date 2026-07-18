// This crate exists only to host trybuild compile-fail tests.
//
// The `proto` module is real tonic-build output (compiled from
// `proto/ping.proto` by `r2e-grpc-build` in build.rs): the gRPC fixtures
// reference `r2e_compile_tests::proto::ping::…` so they typecheck against
// the genuine generated surface — no hand-written tonic stand-in to drift
// on tonic bumps.
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/r2e_protos.rs"));
}
