//! Regenerates the checked-in OpenFGA gRPC client.
//!
//! `r2e-openfga` deliberately has **no `build.rs`**: a build script would make
//! `protoc` a hard requirement on every machine that merely enables the
//! `openfga` feature, even though such a consumer never authors a proto. So the
//! generated client is committed under `r2e-openfga/src/proto/` and this binary
//! is the only thing that needs `protoc`.
//!
//! Run it through `scripts/generate-openfga-proto.sh`, which also checks the
//! result is in sync (CI runs the same script with `--check`).

use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("codegen lives under r2e-openfga/")
        .to_path_buf();

    let proto_dir = crate_dir.join("proto");
    let out_dir = crate_dir.join("src").join("proto");
    std::fs::create_dir_all(&out_dir)?;

    let protos = [
        "proto/openfga/v1/authzmodel.proto",
        "proto/openfga/v1/errors_ignore.proto",
        "proto/openfga/v1/openapi.proto",
        "proto/openfga/v1/openfga.proto",
        "proto/openfga/v1/openfga_service.proto",
    ]
    .map(|p| crate_dir.join(p));

    // Client only: R2E consumes OpenFGA, it never serves the API.
    //
    // No `serde` derive and no `prost-wkt` extern paths, unlike the upstream
    // `openfga-rs` build script we replaced. The derived serde was unusable
    // anyway — prost tags oneof variants by Rust variant name, which does not
    // match OpenFGA's JSON — so `model_convert` converts the AST by hand and
    // nothing in the crate ever serialized a wire type.
    tonic_prost_build::configure()
        .build_server(false)
        .build_client(true)
        .out_dir(&out_dir)
        .compile_protos(&protos, &[proto_dir])?;

    // `google/api`, `validate` and the openapiv2 options are pulled in only for
    // the annotations OpenFGA's protos carry; prost emits a module for each of
    // them that nothing references. Drop them so the committed tree holds
    // exactly the one file we include.
    for entry in std::fs::read_dir(&out_dir)? {
        let path = entry?.path();
        if path.file_name().is_some_and(|n| n != "openfga.v1.rs") {
            std::fs::remove_file(&path)?;
        }
    }

    println!("generated {}", out_dir.join("openfga.v1.rs").display());
    Ok(())
}
