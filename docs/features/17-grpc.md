# Feature 17 — gRPC

## TL;DR

Native gRPC with the same developer experience as HTTP controllers: `#[inject]`, `#[config]`, and interceptors, all wired at compile time. Proto setup is automagic: a one-line `build.rs` (`r2e_grpc_build::compile()`) compiles every `.proto` under `proto/` and `r2e::r2e_grpc::include_protos!()` includes the generated modules (plus a combined `FILE_DESCRIPTOR_SET` for reflection) — `r2e add grpc` scaffolds the whole thing. Annotate a service impl with `#[grpc_routes]` (analogous to `#[routes]`) — construction goes through `ContextConstruct` from the bean graph. Install the `GrpcServer` plugin before `build_state()`; transport is either a dedicated port or multiplexed on the HTTP port. A vertical slice can own its services with `#[module(grpc_services(...))]`, which is the only way for a service to inject the module's private beans.


## Goal

Provide native gRPC support with the same developer experience as HTTP controllers: `#[inject]`, `#[config]`, and interceptors — all wired at compile time.

## Key Concepts

### GrpcServer

Plugin to install in `AppBuilder` before `build_state()`. Two transport modes are available: dedicated port or multiplexed on the same port as HTTP.

### `#[grpc_routes]`

Attribute macro analogous to `#[routes]` for gRPC services. It generates a wrapper that implements the tonic service trait, with controller construction via `ContextConstruct` (built from the bean context, by type) and interceptor wrapping.

### Dependency Injection

gRPC services support the same injection as HTTP controllers: `#[inject]` for app-scoped fields and `#[config("key")]` for configuration.

## Usage

### 1. Configuration

> `r2e add grpc` scaffolds everything in this section (plus `build.rs`, a
> sample `proto/greeter.proto`, and a `src/grpc.rs` service skeleton).

Enable the gRPC feature (`grpc-reflection` too if you want `grpcurl list`
to work out of the box):

```toml
r2e = { version = "0.3", features = ["grpc", "grpc-reflection"] }
```

Add the runtime dependencies the generated proto code references, and the
`r2e-grpc-build` build helper:

```toml
[dependencies]
tonic = "~0.14"
tonic-prost = "~0.14"
prost = "~0.14"

[build-dependencies]
r2e-grpc-build = "0.3"
```

### 2. Defining a Proto File

Create a `proto/` directory at the project root with the service definition:

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

### 3. Build Script

One line — `r2e_grpc_build::compile()` finds every `.proto` under `proto/`
(recursively), compiles them, and emits an aggregated module plus the
combined encoded `FileDescriptorSet` for server reflection. The directory is
registered with `cargo:rerun-if-changed`, so dropping a new `.proto` file is
all it takes — `build.rs` never changes again:

```rust
// build.rs
fn main() -> Result<(), Box<dyn std::error::Error>> {
    r2e_grpc_build::compile()
}
```

The generated code produces a server trait (`greeter_server::Greeter`) and message types (`HelloRequest`, `HelloReply`).

For custom setups, `ProtoCompiler` exposes the knobs:

```rust
// build.rs
fn main() -> Result<(), Box<dyn std::error::Error>> {
    r2e_grpc_build::ProtoCompiler::new()
        .proto_dir("api/proto")                                        // default: proto/
        .configure(|b| b.type_attribute(".", "#[derive(serde::Serialize)]"))  // tonic_prost_build::Builder
        .compile()
}
```

### 4. Including the Generated Code

`include_protos!()` brings in everything the build script generated: one Rust
module per proto package (dotted packages become nested modules) and a
`FILE_DESCRIPTOR_SET` constant for reflection:

```rust
pub mod proto {
    r2e::r2e_grpc::include_protos!();
}

use proto::greeter::{HelloReply, HelloRequest};   // package `greeter` → module `greeter`
```

### 5. Implementing a gRPC Service

A gRPC service in R2E follows the same pattern as an HTTP controller:

1. Define a struct with bare `#[controller]` — a gRPC service is state-only (no path); its `#[inject]` fields resolve from the bean graph by type
2. Implement the tonic service trait with `#[grpc_routes(path::to::ServiceTrait)]`

```rust
use r2e::prelude::*;
use r2e::r2e_grpc::{GrpcServer, AppBuilderGrpcExt};

#[controller]
pub struct GreeterService {
    #[inject] greeting_prefix: GreetingPrefix,
}

#[grpc_routes(proto::greeter::greeter_server::Greeter)]
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

#### What `#[grpc_routes]` Generates

The macro produces:

- A wrapper struct `__R2eGrpc_<Name>` holding the retained `Arc<BeanContext>`
- An `#[async_trait]` implementation of the tonic service trait (e.g., `Greeter`)
- An implementation of the `GrpcService` trait (no state generic) that wires everything into the builder via `add_to_routes(Routes, &Arc<BeanContext>)` — each registered service folds into a single `tonic::service::Routes` collection, drained once by the `GrpcServer` plugin at serve time

Each method goes through the pipeline: controller construction via `ContextConstruct` (from the bean context, by type), interceptor wrapping, then the method body.

### 6. Dependency Injection

gRPC services support the same injection as HTTP controllers:

```rust
#[controller]
pub struct UserGrpcService {
    #[inject] user_service: UserService,
    #[inject] event_bus: LocalEventBus,
    #[config("app.name")] app_name: String,
}
```

- `#[inject]` — resolved from the bean graph by type (services, pools, shared types)
- `#[config("key")]` — resolved from `R2eConfig` (itself a bean in the graph)

The service core is constructed once from the bean context via `ContextConstruct` (by type) and shared across requests.

### 7. Interceptors

Apply interceptors at the impl or method level, just like for HTTP:

```rust
#[grpc_routes(proto::greeter::greeter_server::Greeter)]
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

Interceptors wrap the method body, not the tonic handler. The wrapping order is the same as for HTTP: logged > timed > user-defined > method body.

### 8. Plugin Installation

The `GrpcServer` plugin must be installed **before** `build_state()`:

```rust
use r2e::r2e_grpc::{GrpcServer, AppBuilderGrpcExt};

#[r2e::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    r2e::init_tracing();

    let prefix = GreetingPrefix("Hello".to_string());

    let app = AppBuilder::new()
        .plugin(GrpcServer::on_port("0.0.0.0:50051"))
        .provide(prefix)
        .build_state()
        .await
        .register_grpc_service::<GreeterService>();

    tracing::info!("HTTP sur :3000, gRPC sur :50051");
    app.serve("0.0.0.0:3000").await
}
```

`build_state()` takes no type arguments and is async — `.await` it. Register gRPC services **after** `build_state()` with `.register_grpc_service::<S>()` — the gRPC analog of `.register_controller()` for HTTP (both are extension-trait methods on the built phase). Like its HTTP counterpart, it compile-checks the service's dependencies: a bean read by an `#[inject]` field or an `#[intercept(...)]` spec but never provided is a compile error at the registration line, not a startup panic.

### 9. Services owned by a feature module

A vertical slice declares its gRPC services next to its controllers, with
`grpc_services(...)` in `#[module]`:

```rust
#[derive(Clone)]
pub struct GreetingRepo;          // module-private: never exported

#[bean]
impl GreetingRepo {
    fn new() -> Self { Self }
}

#[controller]
pub struct GreeterService {
    #[inject] repo: GreetingRepo, // a PRIVATE bean of the module
}

#[grpc_routes(proto::greeter::greeter_server::Greeter)]
impl GreeterService { /* ... */ }

#[module(
    providers(GreetingRepo),
    grpc_services(GreeterService),
    controllers(GreetingController),   // HTTP routes of the same slice
)]
pub struct GreetingModule;

let app = AppBuilder::new()
    .plugin(GrpcServer::on_port("0.0.0.0:50051"))
    .register_module::<GreetingModule>()   // no .register_grpc_service:: needed
    .build_state()
    .await;
```

This is the only way for a gRPC service to inject a module's **private** beans:
`.register_grpc_service::<S>()` checks the service's dependencies against the
*application state*, so its deps would have to be exported. A module-owned
service is checked **module-locally** at `register_module` (deps ⊆ providers ∪
imports, `#[intercept(...)]` spec deps included) and registered by
`build_state()` from the module's retained bean context, right after the
module's controllers.

`grpc_services(...)` implies the `GrpcServer` plugin — the macro appends it to
the module's `requires_plugins`, so forgetting `.plugin(GrpcServer::...)` before
the module is a compile error naming `GrpcServer`. A module may also bring the
plugin itself: `plugins(GrpcServer = GrpcServer::on_port("0.0.0.0:50051"))`.

Hand-written `FeatureModule` impls (no macro) must write `type Endpoints = ();`
when the module owns no gRPC service.

## Transport Modes

### Separate Port (recommended)

gRPC runs on a dedicated port, HTTP on another:

```rust
AppBuilder::new()
    .plugin(GrpcServer::on_port("0.0.0.0:50051"))
    // ...
    .serve("0.0.0.0:3000")  // HTTP sur :3000
```

When `serve()` starts, the plugin binds the configured address and spawns the tonic server next to the HTTP one (graceful shutdown is tied to the application's shutdown sequence). Clients connect directly to the gRPC port:

```bash
grpcurl -plaintext -d '{"name":"World"}' localhost:50051 greeter.Greeter/SayHello
```

This is the simplest configuration and recommended for most deployments. It avoids content-type routing overhead and allows independent load balancing per protocol.

### Multiplexed (single port)

gRPC and HTTP share the same port, routed by the `content-type` header:

```rust
AppBuilder::new()
    .plugin(GrpcServer::multiplexed())
    // ...
    .serve("0.0.0.0:3000")  // HTTP et gRPC sur :3000
```

Requests with `content-type: application/grpc` (`+proto`, `+json`, …) are routed to the gRPC services; `application/grpc-web*` to the grpc-web arm (see below); others go to the Axum HTTP router. Routing is handled by `MultiplexService`, a Tower service that inspects the content-type header, mounted around the assembled HTTP router at build time.

Since gRPC requires HTTP/2, plaintext clients must use h2c prior knowledge (tonic's default); the HTTP server accepts both HTTP/1.1 and h2c on the shared port.

Use this when infrastructure constraints require a single port (e.g., certain PaaS environments).

### grpc-web (browser clients)

Browser clients (grpc-web, connect-web) speak grpc-web, not gRPC: HTTP/1.1 or HTTP/2, an optional base64 `-text` encoding, and the trailers folded into a final body frame. By default the multiplexer answers `application/grpc-web*` requests with `415 Unsupported Media Type` and logs a warning at boot — never a garbled raw-gRPC response.

Enable it with the `grpc-web` feature (`web` on `r2e-grpc`) and `with_grpc_web()`:

```rust
AppBuilder::new()
    .plugin(GrpcServer::multiplexed().with_grpc_web())
```

The grpc-web arm is `tonic-web` around the same registered routes (interceptors, guards, and reflection included), fronted by a CORS layer. The default policy allows any origin, `POST`/`OPTIONS`, mirrors the requested headers, exposes `grpc-status` / `grpc-message` / `grpc-status-details-bin`, and caches preflights for 24h. Bring your own with `with_grpc_web_cors(CorsLayer)` (a `tower_http::cors::CorsLayer`). grpc-web preflights — `OPTIONS` whose `access-control-request-headers` names `x-grpc-web` — are routed to that layer, so the app needs no `Cors` plugin for browser gRPC; other preflights keep going to the HTTP router.

`with_grpc_web()` is only honoured by the multiplexed transport; on `on_port` it is ignored with a boot warning. With the feature off nothing changes (no `tonic-web` dependency, 415 as before).

## Combining HTTP and gRPC

A single application can serve HTTP controllers and gRPC services:

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

HTTP controllers use `#[routes]`, gRPC services use `#[grpc_routes]`. Both draw from the same bean graph (`Arc<BeanContext>`) — the same dependency injection graph.

## gRPC Reflection

Enable server reflection for introspection tools like `grpcurl` and gRPC UI. Reflection needs the encoded `FileDescriptorSet` of your protos — `r2e_grpc_build::compile()` already emits it, and `include_protos!()` already exposes it as `proto::FILE_DESCRIPTOR_SET`. Two pieces remain — declare the set on the service and enable reflection on the plugin:

```rust
#[grpc_routes(proto::greeter::greeter_server::Greeter, descriptor = proto::FILE_DESCRIPTOR_SET)]
impl GreeterService { /* ... */ }

AppBuilder::new()
    .plugin(GrpcServer::on_port("0.0.0.0:50051").with_reflection())
```

Each `register_grpc_service` contributes its service's descriptor set to the reflection registry (identical sets are stored once). For descriptor sets not carried by a service, use `with_reflection_descriptor(bytes)` — it also implies `with_reflection()`. Both reflection protocol versions (v1 and v1alpha) are served, on both transports (separate port and multiplexed).

This requires the `reflection` feature on `r2e-grpc` (`grpc-reflection` on the `r2e` facade) — without it, `with_reflection()` does not exist and the build fails instead of silently serving no reflection:

```toml
[dependencies]
r2e = { version = "0.3", features = ["grpc", "grpc-reflection"] }
```

With reflection enabled, clients can discover services without the proto file:

```bash
grpcurl -plaintext localhost:50051 list
grpcurl -plaintext localhost:50051 describe greeter.Greeter
```

## Configuration

The gRPC port can be configured via `application.yaml`:

```yaml
grpc:
  port: 50051
```

And referenced in the setup:

```rust
let config = R2eConfig::load().unwrap();
let grpc_port = config.get_or("grpc.port", "50051".to_string());
let grpc_addr = format!("0.0.0.0:{grpc_port}");

AppBuilder::new()
    .plugin(GrpcServer::on_port(grpc_addr))
```

## Prerequisites

- gRPC service structs must **not** have `#[inject(identity)]` fields at the struct level: gRPC services construct from the bean context outside any HTTP request (via `ContextConstruct`), where no request identity is available. Use handler-level identity if needed.
- The `GrpcServer` plugin must be installed before `build_state()`
- Proto files are compiled at build time — changes to `.proto` files require a `cargo build`

## CLI Scaffolding

`r2e add grpc` sets up gRPC in an existing project: enables the
`grpc`/`grpc-reflection` features on the `r2e` dependency, adds the
`tonic`/`tonic-prost`/`prost` dependencies and the `r2e-grpc-build`
build-dependency, and drops the one-line `build.rs`, a sample
`proto/greeter.proto`, and a `src/grpc/` module (`mod.rs` holds the single
shared `proto` module; one file per service reaches it via `super::proto`)
— the samples only if the project has no protos yet. `r2e new --grpc`
scaffolds the same, pre-wired into `App::build`.

For additional services:

```bash
r2e generate grpc-service User --package myapp
```

This creates:
- `proto/user.proto` — proto file with CRUD RPC definitions
- `src/grpc/user.rs` — Rust service implementation skeleton

The `--package` flag sets the protobuf package name (defaults to `myapp`).

After generation, you need to:
1. Register in `src/app.rs`: `.register_grpc_service::<UserService>()`
2. Run `cargo build` — build.rs picks up the new proto automatically

## GrpcServer API Reference

| Constructor | Description |
|-------------|-------------|
| `GrpcServer::on_port(addr)` | gRPC on a separate port |
| `GrpcServer::multiplexed()` | gRPC and HTTP on the same port |
| `.with_grpc_web()` / `.with_grpc_web_cors(cors)` | grpc-web (tonic-web) on the multiplexed port, feature `grpc-web` |

| Method | Description |
|--------|-------------|
| `.with_reflection()` | Enable gRPC server reflection, v1 + v1alpha (requires the `reflection` feature) |
| `.with_reflection_descriptor(bytes)` | Enable reflection and register an extra encoded `FileDescriptorSet` |

| Builder Method | Description |
|----------------|-------------|
| `.register_grpc_service::<S>()` | Register a gRPC service (analog of `register_controller`) |
| `#[module(grpc_services(S))]` | A feature module owns the service — registered by `build_state()`, may inject the module's private beans |

## Supported Decorators

| Decorator | Status | Description |
|-----------|--------|-------------|
| `#[intercept(...)]` | Supported | Interceptors (impl and method level) |
| `#[roles(...)]` | Planned | Role-based guards |
| `#[guard(...)]` | Planned | Custom guards |
| `#[inject(identity)]` | Planned | Identity extraction from metadata |

The guard and identity infrastructure exists in `r2e-grpc` (`GrpcGuard`, `GrpcGuardContext`, `GrpcRolesGuard`, `GrpcIdentityExtractor`) and will be enabled in a future version.

## Validation Criteria

Launch the application and test the gRPC service:

```bash
grpcurl -plaintext -d '{"name":"World"}' localhost:50051 greeter.Greeter/SayHello
# → {"message":"Hello World!"}
```
