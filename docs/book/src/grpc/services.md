# gRPC Services

R2E provides first-class gRPC support with the same developer experience as HTTP controllers: `#[inject]`, `#[config]`, and interceptors — all wired at compile time.

## Setup

Enable the gRPC feature:

```toml
r2e = { version = "0.1", features = ["grpc"] }
```

Add `tonic-build` and `prost` for proto compilation:

```toml
[dependencies]
tonic = "0.12"
prost = "0.13"

[build-dependencies]
tonic-build = "0.12"
```

## Defining a proto file

Create a `proto/` directory at the project root with your service definition:

```protobuf
// proto/greeter.proto
syntax = "proto3";

package greeter;

service Greeter {
  rpc SayHello (HelloRequest) returns (HelloReply);
  rpc SayHelloAdmin (HelloRequest) returns (HelloReply);
}

message HelloRequest {
  string name = 1;
}

message HelloReply {
  string message = 1;
}
```

## Build script

Create a `build.rs` at the project root to compile protos into Rust code:

```rust
// build.rs
fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::compile_protos("proto/greeter.proto")?;
    Ok(())
}
```

The generated code produces a server trait (`greeter_server::Greeter`) and message types (`HelloRequest`, `HelloReply`).

## Including generated code

Use `tonic::include_proto!` to bring the generated types into scope:

```rust
pub mod proto {
    tonic::include_proto!("greeter");
}

use proto::{HelloReply, HelloRequest};
```

The string passed to `include_proto!` must match the `package` name in the `.proto` file.

## Implementing a gRPC service

A gRPC service in R2E follows the same pattern as an HTTP controller:

1. Define a struct with `#[controller]` (no `state =` — the service resolves its `#[inject]` fields by type from the bean graph)
2. Implement the tonic service trait with `#[grpc_routes(path::to::ServiceTrait)]`

```rust
use r2e::prelude::*;
use r2e::r2e_grpc::{GrpcServer, AppBuilderGrpcExt};

#[controller]
pub struct GreeterService {
    #[inject] greeting_prefix: GreetingPrefix,
}

#[grpc_routes(proto::greeter_server::Greeter)]
impl GreeterService {
    async fn say_hello(
        &self,
        request: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        let name = &request.get_ref().name;
        let reply = HelloReply {
            message: format!("{} {}!", self.greeting_prefix.0, name),
        };
        Ok(tonic::Response::new(reply))
    }

    async fn say_hello_admin(
        &self,
        request: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        let name = &request.get_ref().name;
        let reply = HelloReply {
            message: format!("[ADMIN] {} {}!", self.greeting_prefix.0, name),
        };
        Ok(tonic::Response::new(reply))
    }
}
```

### What `#[grpc_routes]` generates

The macro produces:

- A wrapper struct `__R2eGrpc_<Name>` that holds an `Arc` to the service core
- An `#[async_trait]` implementation of the tonic service trait (e.g., `Greeter`)
- A `GrpcService` trait implementation that wires everything into the builder (no state generic — it constructs from the `BeanContext` and exposes `add_to_routes(Routes, &Arc<BeanContext>)`; each registered service folds into a single `tonic::service::Routes` collection, drained once by the `GrpcServer` plugin at serve time)

Each method goes through the pipeline: core construction via `ContextConstruct` (`from_context` pulls each `#[inject]` field by type from the resolved bean graph, once at registration), interceptor wrapping, then the method body.

## Dependency injection

gRPC services support the same injection as HTTP controllers:

```rust
#[controller]
pub struct UserGrpcService {
    #[inject] user_service: UserService,
    #[inject] event_bus: LocalEventBus,
    #[config("app.name")] app_name: String,
}
```

- `#[inject]` — resolved by type from the bean graph (services, pools, shared types)
- `#[config("key")]` — resolves from `R2eConfig` (itself a bean in the graph)

The service core is constructed once from the resolved `BeanContext` via `ContextConstruct` and shared as an `Arc` across all requests. A `#[inject]` type that is not a registered bean is a **compile error** naming the missing type.

## Interceptors

Apply interceptors at the impl level or method level, same as HTTP:

```rust
#[grpc_routes(proto::greeter_server::Greeter)]
#[intercept(Logged::info())]
impl GreeterService {
    #[intercept(Timed::default())]
    async fn say_hello(
        &self,
        request: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        // ...
    }
}
```

Interceptors wrap the method body, not the tonic handler. The wrapping order is the same as HTTP: logged > timed > user-defined > method body.

## Installing the plugin

The `GrpcServer` plugin must be installed **before** `build_state()`:

```rust
use r2e::r2e_grpc::{GrpcServer, AppBuilderGrpcExt};

#[r2e::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let prefix = GreetingPrefix("Hello".to_string());

    let app = AppBuilder::new()
        .plugin(GrpcServer::on_port("0.0.0.0:50051"))
        .provide(prefix)
        .build_state()
        .await
        .register_grpc_service::<GreeterService>();

    tracing::info!("HTTP on :3000, gRPC on :50051");
    app.serve("0.0.0.0:3000").await
}
```

`.provide(...)` and `.register::<T>()` populate the bean graph, `build_state().await` resolves it into the inferred state (no hand-written state struct), and `.register_grpc_service::<S>()` then constructs the service core from that graph.

Register gRPC services with `.register_grpc_service::<S>()` — the gRPC analog of `.register_controller()` for HTTP, including the compile-time dependency check: if the service's `#[inject]` fields or its `#[intercept(...)]` specs read a bean the app never provided, the registration line fails to compile with a `missing .provide::<TheBean>()` diagnostic — exactly like an HTTP controller.

## Transport modes

R2E supports two ways to expose gRPC:

### Separate port (recommended)

gRPC runs on a dedicated port, HTTP on another:

```rust
AppBuilder::new()
    .plugin(GrpcServer::on_port("0.0.0.0:50051"))
    // ...
    .serve("0.0.0.0:3000")  // HTTP on :3000
```

When `serve()` starts, the plugin binds the configured address and spawns the tonic server next to the HTTP one; graceful shutdown is tied to the application's shutdown sequence. Clients connect to the gRPC port directly:

```bash
grpcurl -plaintext -d '{"name":"World"}' localhost:50051 greeter.Greeter/SayHello
```

This is the simplest setup and recommended for most deployments. It avoids content-type routing overhead and allows independent load balancing per protocol.

### Multiplexed (single port)

gRPC and HTTP share the same port, routed by the `content-type` header:

```rust
AppBuilder::new()
    .plugin(GrpcServer::multiplexed())
    // ...
    .serve("0.0.0.0:3000")  // both HTTP and gRPC on :3000
```

Requests with `content-type: application/grpc*` are routed to the gRPC services; all others go to the Axum HTTP router. The routing is handled by `MultiplexService`, a Tower service that inspects the content-type header, mounted around the assembled HTTP router at build time.

Since gRPC requires HTTP/2, plaintext clients must use h2c prior knowledge (tonic's default); the HTTP server accepts both HTTP/1.1 and h2c on the shared port.

Use this when infrastructure constraints require a single port (e.g., certain PaaS environments).

## Combining HTTP and gRPC

A single application can serve both HTTP controllers and gRPC services:

```rust
let app = AppBuilder::new()
    .plugin(GrpcServer::on_port("0.0.0.0:50051"))
    .provide(prefix)
    .build_state()
    .await
    .register_grpc_service::<GreeterService>()    // gRPC
    .register_controller::<HealthController>()     // HTTP
    .register_controller::<UserController>();       // HTTP

app.serve("0.0.0.0:3000").await
```

HTTP controllers use `#[routes]`, gRPC services use `#[grpc_routes]`. Both construct their cores from the same resolved bean graph.

## gRPC reflection

Enable server reflection for introspection tools like `grpcurl` and gRPC UI. Reflection needs the encoded `FileDescriptorSet` of your protos, so three pieces cooperate:

1. **build.rs** — emit the descriptor set next to the generated code:

```rust
tonic_prost_build::configure()
    .file_descriptor_set_path(out_dir.join("greeter_descriptor.bin"))
    .compile_protos(&["proto/greeter.proto"], &["proto"])?;
```

2. **Proto module** — expose the bytes:

```rust
pub mod proto {
    tonic::include_proto!("greeter");
    pub const FILE_DESCRIPTOR_SET: &[u8] =
        tonic::include_file_descriptor_set!("greeter_descriptor");
}
```

3. **Service + plugin** — declare the set on the service and enable reflection:

```rust
#[grpc_routes(proto::greeter_server::Greeter, descriptor = proto::FILE_DESCRIPTOR_SET)]
impl GreeterService { /* ... */ }

AppBuilder::new()
    .plugin(GrpcServer::on_port("0.0.0.0:50051").with_reflection())
```

Each `register_grpc_service` contributes its service's descriptor set to the reflection registry (identical sets are stored once). For descriptor sets not carried by a service, use `with_reflection_descriptor(bytes)` — it also implies `with_reflection()`. Both reflection protocol versions (v1 and v1alpha) are served, on both transports (separate port and multiplexed).

This requires the `reflection` feature on `r2e-grpc` (`grpc-reflection` on the `r2e` facade) — without it, `with_reflection()` does not exist and the build fails instead of silently serving no reflection:

```toml
[dependencies]
r2e = { version = "0.1", features = ["grpc", "grpc-reflection"] }
```

With reflection enabled, clients can discover services without the proto file:

```bash
grpcurl -plaintext localhost:50051 list
grpcurl -plaintext localhost:50051 describe greeter.Greeter
```

## Configuration

You can configure the gRPC port via `application.yaml`:

```yaml
grpc:
  port: 50051
```

And reference it in your setup:

```rust
let config = R2eConfig::load().unwrap();
let grpc_port = config.get_or("grpc.port", "50051".to_string());
let grpc_addr = format!("0.0.0.0:{grpc_port}");

AppBuilder::new()
    .plugin(GrpcServer::on_port(grpc_addr))
```

## Requirements

- gRPC service structs must **not** have struct-level `#[inject(identity)]` fields — the core is built from the bean context via `ContextConstruct`, which has no request to extract identity from (use param-level identity once macro support lands)
- The `GrpcServer` plugin must be installed before `build_state()`
- Proto files are compiled at build time — changes to `.proto` files require a `cargo build`

## CLI scaffolding

The R2E CLI can generate gRPC service skeletons:

```bash
r2e generate grpc-service User --package myapp
```

This creates:
- `proto/user.proto` — proto file with CRUD RPC definitions
- `src/grpc/user.rs` — Rust service implementation skeleton

The `--package` flag sets the protobuf package name (defaults to `myapp`).

After generating, you need to:
1. Add the proto to `build.rs`: `tonic_build::compile_protos("proto/user.proto")?;`
2. Register in `src/app.rs`: `.register_grpc_service::<UserService>()`
3. Run `cargo build` to generate proto code

See [CLI Reference](../reference/cli-reference.md) for full details.

## GrpcServer API

| Constructor | Description |
|-------------|-------------|
| `GrpcServer::on_port(addr)` | gRPC on a separate port |
| `GrpcServer::multiplexed()` | gRPC and HTTP on the same port |

| Method | Description |
|--------|-------------|
| `.with_reflection()` | Enable gRPC server reflection, v1 + v1alpha (requires `reflection` feature) |
| `.with_reflection_descriptor(bytes)` | Enable reflection and register an extra encoded `FileDescriptorSet` |

| Builder method | Description |
|----------------|-------------|
| `.register_grpc_service::<S>()` | Register a gRPC service (analog of `register_controller`) |

## Supported decorators

Currently, `#[grpc_routes]` supports:

| Decorator | Status | Description |
|-----------|--------|-------------|
| `#[intercept(...)]` | Supported | Interceptors (impl and method level) |
| `#[roles(...)]` | Planned | Role-based guards |
| `#[guard(...)]` | Planned | Custom guards |
| `#[inject(identity)]` | Planned | Identity extraction from metadata |

The guard and identity infrastructure exists in `r2e-grpc` (`GrpcGuard`, `GrpcGuardContext`, `GrpcRolesGuard`, `GrpcIdentityExtractor`) and will be enabled in a future release. See [Guards and Identity](./guards-and-identity.md) for the API design.

## Next steps

- [Guards and Identity](./guards-and-identity.md) — guard and identity API design for gRPC
- [Interceptors](../advanced/interceptors.md) — interceptor patterns (shared with HTTP)
- [CLI Reference](../reference/cli-reference.md) — `r2e generate grpc-service`
