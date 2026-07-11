fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Emit the encoded FileDescriptorSet next to the generated code so the
    // proto modules can expose it for gRPC server reflection
    // (`tonic::include_file_descriptor_set!` in `main.rs` / the tests).
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR")?);
    tonic_prost_build::configure()
        .file_descriptor_set_path(out_dir.join("greeter_descriptor.bin"))
        .compile_protos(&["proto/greeter.proto"], &["proto"])?;
    Ok(())
}
