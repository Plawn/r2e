# r2e-grpc-build

Build-script helper for [`r2e-grpc`](../README.md): compiles every `.proto` under `proto/` in one line.

## Overview

`r2e_grpc_build::compile()` finds every `.proto` file under `proto/` (recursively), compiles them with `tonic-prost-build`, and emits:

- an aggregated module with one Rust module per proto package (dotted packages become nested modules);
- the combined encoded `FileDescriptorSet` for server reflection.

The proto directory is registered with `cargo:rerun-if-changed`, so adding a `.proto` file is all it takes — `build.rs` never changes again.

## Usage

```toml
[build-dependencies]
r2e-grpc-build = "0.6"
```

```rust
// build.rs
fn main() -> Result<(), Box<dyn std::error::Error>> {
    r2e_grpc_build::compile()
}
```

Include the generated code with `include_protos!()` from `r2e-grpc` (facade feature `grpc`):

```rust
pub mod proto {
    r2e::r2e_grpc::include_protos!();
}
```

### Custom setups

`ProtoCompiler` exposes the knobs:

```rust
// build.rs
fn main() -> Result<(), Box<dyn std::error::Error>> {
    r2e_grpc_build::ProtoCompiler::new()
        .proto_dir("api/proto")                                              // default: proto/
        .configure(|b| b.type_attribute(".", "#[derive(serde::Serialize)]"))  // tonic_prost_build::Builder
        .compile()
}
```

## Key types

| Item | Description |
|------|-------------|
| `compile()` | Compile `proto/` with defaults |
| `ProtoCompiler` | Builder: `proto_dir`, `configure` (access to the underlying `tonic_prost_build::Builder`), `compile` |

## Documentation

Full gRPC guide: [`docs/features/17-grpc.md`](../../docs/features/17-grpc.md).

## License

Apache-2.0
