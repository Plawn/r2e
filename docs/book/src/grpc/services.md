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

1. Define a struct with `#[derive(Controller)]` and `#[controller(state = ...)]`
2. Implement the tonic service trait with `#[grpc_routes(path::to::ServiceTrait)]`

```rust
use r2e::prelude::*;
use r2e::r2e_grpc::{GrpcServer, AppBuilderGrpcExt};

#[derive(Controller)]
#[controller(state = Services)]
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

- A wrapper struct `__R2eGrpc_<Name>` that holds the app state
- An `#[async_trait]` implementation of the tonic service trait (e.g., `Greeter`)
- A `GrpcService<T>` trait implementation that wires everything into the builder

Each method goes through the pipeline: controller construction via `StatefulConstruct`, interceptor wrapping, then the method body.

## Dependency injection

gRPC services support the same injection as HTTP controllers:

```rust
#[derive(Controller)]
#[controller(state = AppState)]
pub struct UserGrpcService {
    #[inject] user_service: UserService,
    #[inject] event_bus: EventBus,
    #[config("app.name")] app_name: String,
}
```

- `#[inject]` — clones from app state (services, pools, shared types)
- `#[config("key")]` — resolves from `R2eConfig`

The controller is constructed from state via `StatefulConstruct` for each request.

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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    r2e::init_tracing();

    let prefix = GreetingPrefix("Hello".to_string());

    let app = AppBuilder::new()
        .plugin(GrpcServer::on_port("0.0.0.0:50051"))
        .provide(prefix)
        .build_state::<Services, _>()
        .await
        .register_grpc_service::<GreeterService>();

    tracing::info!("HTTP on :3000, gRPC on :50051");
    app.serve("0.0.0.0:3000").await
}
```

Register gRPC services with `.register_grpc_service::<S>()` — the gRPC analog of `.register_controller()` for HTTP.

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

Clients connect to the gRPC port directly:

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

Requests with `content-type: application/grpc*` are routed to the gRPC server; all others go to the Axum HTTP router. The routing is handled by `MultiplexService`, a Tower service that inspects the content-type header.

Use this when infrastructure constraints require a single port (e.g., certain PaaS environments).

## Combining HTTP and gRPC

A single application can serve both HTTP controllers and gRPC services:

```rust
let app = AppBuilder::new()
    .plugin(GrpcServer::on_port("0.0.0.0:50051"))
    .provide(prefix)
    .build_state::<Services, _>()
    .await
    .register_grpc_service::<GreeterService>()    // gRPC
    .register_controller::<HealthController>()     // HTTP
    .register_controller::<UserController>();       // HTTP

app.serve("0.0.0.0:3000").await
```

HTTP controllers use `#[routes]`, gRPC services use `#[grpc_routes]`. Both share the same app state and dependency injection graph.

## gRPC reflection

Enable server reflection for introspection tools like `grpcurl` and gRPC UI:

```rust
AppBuilder::new()
    .plugin(GrpcServer::on_port("0.0.0.0:50051").with_reflection())
```

This requires the `reflection` feature on `r2e-grpc`:

```toml
[dependencies]
r2e = { version = "0.1", features = ["grpc"] }
r2e-grpc = { version = "0.1", features = ["reflection"] }
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
let config = R2eConfig::load("dev");
let grpc_port = config.get_or("grpc.port", "50051".to_string());
let grpc_addr = format!("0.0.0.0:{grpc_port}");

AppBuilder::new()
    .plugin(GrpcServer::on_port(grpc_addr))
```

## Requirements

- gRPC service structs must **not** have struct-level `#[inject(identity)]` fields (they require `StatefulConstruct`)
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
2. Register in `main.rs`: `.register_grpc_service::<UserService>()`
3. Run `cargo build` to generate proto code

See [CLI Reference](../reference/cli-reference.md) for full details.

## GrpcServer API

| Constructor | Description |
|-------------|-------------|
| `GrpcServer::on_port(addr)` | gRPC on a separate port |
| `GrpcServer::multiplexed()` | gRPC and HTTP on the same port |

| Method | Description |
|--------|-------------|
| `.with_reflection()` | Enable gRPC server reflection (requires `reflection` feature) |

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
