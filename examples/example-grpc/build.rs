fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Compiles every proto under `proto/` and generates the aggregated
    // module (+ combined FileDescriptorSet for gRPC server reflection)
    // consumed via `r2e::r2e_grpc::include_protos!()`.
    r2e_grpc_build::compile()
}
