//! The OpenFGA gRPC client, generated from `r2e-openfga/proto/openfga/v1/*.proto`.
//!
//! The generated code is **committed** (`src/proto/openfga.v1.rs`) rather than
//! produced by a `build.rs`. A build script would make `protoc` a hard
//! requirement for anyone enabling the `openfga` feature, even though such a
//! consumer never authors a proto of their own; committing the output keeps the
//! crate buildable with nothing but cargo.
//!
//! Regenerate with `scripts/generate-openfga-proto.sh` after touching a
//! `.proto`. CI runs the same script with `--check` and fails on drift, so the
//! file can never silently diverge from the schema.
//!
//! Only the client is generated (`build_server(false)`): R2E consumes OpenFGA,
//! it never serves the API.

include!("proto/openfga.v1.rs");
