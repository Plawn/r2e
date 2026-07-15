# r2e-grpc

gRPC server support for R2E — Tonic-based, with the same DX as HTTP controllers.

## Overview

Host gRPC services alongside HTTP controllers with full access to R2E's DI, guards, interceptors, and identity extraction. Supports separate-port and multiplexed (same-port) transport modes.

## Usage

Via the facade crate:

```toml
[dependencies]
r2e = { version = "0.1", features = ["grpc"] }
```

### Separate port

```rust
use r2e_grpc::GrpcServer;

AppBuilder::new()
    .plugin(GrpcServer::on_port("0.0.0.0:50051"))
    .build_state()
    .await
    .register_grpc_service::<UserGrpcService>()
    .serve("0.0.0.0:3000")   // HTTP on 3000, gRPC on 50051
    .await;
```

### Multiplexed (same port)

```rust
AppBuilder::new()
    .plugin(GrpcServer::multiplexed())
    .build_state()
    .await
    .register_grpc_service::<UserGrpcService>()
    .serve("0.0.0.0:3000")   // both HTTP and gRPC on 3000
    .await;
```

## Key types

| Type | Description |
|------|-------------|
| `GrpcServer` | `PreStatePlugin` — configures transport mode |
| `GrpcService` | Trait implemented by gRPC service structs |
| `AppBuilderGrpcExt` | Extension trait adding `.register_grpc_service::<T>()` |
| `GrpcGuard` / `GrpcGuardContext` | Authorization guards for gRPC methods |
| `GrpcRolesGuard` | Role-based access control for gRPC |
| `GrpcIdentityExtractor` | Extract JWT identity from gRPC metadata |

## Features

- Compile-time dependency check (missing bean = compile error at `register_grpc_service`)
- `#[inject]`, `#[config]` on gRPC service structs
- Guards and interceptors via `#[guard]` / `#[intercept]`
- JWT identity extraction from `authorization` metadata
- Graceful shutdown with drain support

## License

Apache-2.0
