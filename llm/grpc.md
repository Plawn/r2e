---
topic: grpc
features: grpc
tokens: ~1500
requires: core-concepts, modules
---

## gRPC

### TL;DR

- Enable feature `grpc`; declare the service with `#[controller]` +
  `#[grpc_routes(<generated server trait>)]` and register it with
  `.register_grpc_service::<S>()` after `build_state()`.
- `build.rs` is one line — `r2e_grpc_build::compile()` — and it compiles every
  `.proto` under `proto/`; dropping a new file in is enough (rerun-if-changed).
- Include the generated code with `r2e::r2e_grpc::include_protos!()`; one Rust
  module per proto package. Add `tonic`, `tonic-prost` and `prost` as
  dependencies (`r2e add grpc` scaffolds all of it).
- `descriptor = proto::FILE_DESCRIPTOR_SET` on `#[grpc_routes]` is only needed
  for reflection (`GrpcServer::…with_reflection()`).
- Pick the transport: `GrpcServer::on_port("0.0.0.0:50051")` (separate port) or
  `GrpcServer::multiplexed()` (same port as HTTP, routed by `content-type`).
- Browser clients need `.with_grpc_web()` (features `grpc-web` on `r2e`, `web`
  on `r2e-grpc`) — otherwise grpc-web requests get 415; that arm carries its
  own CORS, so no `Cors` plugin is needed.
- `#[guard]`, `#[roles]`, `#[inject(identity)]`, `#[post_construct]`,
  `#[pre_destroy]` and `#[on_start]` are compile errors on `#[grpc_routes]`
  methods; `#[intercept]` is the only decorator supported.
- For auth, wire it by hand: `#[inject] jwt_validator: Arc<JwtClaimsValidator>`
  then `GrpcIdentityExtractor::extract_claims(request.metadata(), &self.jwt_validator)`.
- `register_grpc_service` panics on a missing config key; use
  `try_register_grpc_service::<S>()` to report the failure yourself.
- A service owned by a feature module is registered by
  `#[module(grpc_services(GreeterService))]` — the only way for it to inject
  the module's private beans.

Requires feature: `grpc`. Same DX as HTTP controllers: `#[inject]`, `#[config]`,
interceptors.

Proto setup is one line: the `r2e-grpc-build` build-dependency compiles every
`.proto` under `proto/` (rerun-if-changed — dropping a new file is enough) and
generates an aggregated module (one Rust module per proto package, nested for
dotted packages) plus a combined `FILE_DESCRIPTOR_SET` for server reflection.
The generated code references `::tonic`, `::tonic_prost`, `::prost` — add them
as dependencies (`r2e add grpc` scaffolds all of this).

```rust,ignore
// build.rs
fn main() -> Result<(), Box<dyn std::error::Error>> { r2e_grpc_build::compile() }

// src: include the generated modules
pub mod proto {
    r2e::r2e_grpc::include_protos!();               // expands OUT_DIR/r2e_protos.rs
}
use proto::greeter::{HelloReply, HelloRequest};     // package `greeter` → module `greeter`
```

Customization: `r2e_grpc_build::ProtoCompiler::new().proto_dir("api/proto").configure(|b| /* tonic_prost_build::Builder */ b).compile()`.

```rust
use r2e::r2e_grpc::{GrpcServer, AppBuilderGrpcExt};

# async fn __doc(b: AppBuilder) -> impl Sized {
b.plugin(GrpcServer::on_port("0.0.0.0:50051").with_reflection())  // separate port (or multiplexed)
 .build_state().await
 .register_grpc_service::<GreeterService>()       // deps + config keys checked here
# }
```

`GrpcServer::multiplexed()` serves gRPC and HTTP on the **same** port, routing
by `content-type` (`application/grpc` → tonic, `application/grpc-web*` → the
grpc-web arm, everything else → the HTTP router). By default the grpc-web arm
answers `415 Unsupported Media Type` (+ a boot warning). Enable real grpc-web
(feature `grpc-web` on `r2e`, `web` on `r2e-grpc`) with:

```rust
use tower_http::cors::CorsLayer;

# fn __doc(b: AppBuilder) -> impl Sized {
b.plugin(GrpcServer::multiplexed().with_grpc_web())                  // default CORS: any origin, POST/OPTIONS, grpc-status/-message exposed
# }
# fn __doc2(b: AppBuilder) -> impl Sized {
b.plugin(GrpcServer::multiplexed().with_grpc_web_cors(CorsLayer::new().allow_methods([Method::POST, Method::OPTIONS])))  // your own tower-http CorsLayer
# }
```

That arm is `tonic-web` around the same routes: binary and `-text` (base64),
HTTP/1.1 and HTTP/2, trailer frame included. grpc-web preflights (`OPTIONS`
naming `x-grpc-web`) are routed to that arm's CORS layer, so no `Cors` plugin
is needed for browser clients. Separate-port transport ignores it (warning).

`register_grpc_service` panics on a missing key;
`try_register_grpc_service::<S>()` (same trait) returns
`Result<Self, ConfigValidationError>` for callers that want to report the
failure themselves — the gRPC peer of `try_register_controller`.

A service owned by a feature module is registered by the module instead —
`#[module(grpc_services(GreeterService))]`, see llm/modules.md — which
is the only way for a gRPC service to inject the module's private beans.

```rust
use r2e::r2e_grpc::tonic::{Request, Response, Status};

#[controller]
pub struct GreeterService {
    #[inject] user_service: UserService,
}

#[grpc_routes(proto::greeter::greeter_server::Greeter, descriptor = proto::FILE_DESCRIPTOR_SET)]
impl GreeterService {
    async fn say_hello(&self, request: Request<HelloRequest>) -> Result<Response<HelloReply>, Status> {
        Ok(Response::new(HelloReply { message: format!("Hello, {}!", request.into_inner().name) }))
    }
}
# fn main() {}
```

(`descriptor = …` is optional — only needed for reflection.)

NOT supported on `#[grpc_routes]` methods (compile errors): `#[guard(...)]`,
`#[roles(...)]`, `#[inject(identity)]`, `#[post_construct]`, `#[pre_destroy]`,
`#[on_start]`.
The guard/identity infrastructure (`GrpcGuard`, `GrpcRolesGuard`,
`GrpcIdentityExtractor`) exists in `r2e-grpc` for manual wiring but is not yet
macro-wired. Manual wiring: `#[inject] jwt_validator: Arc<JwtClaimsValidator>`
on the service, then `GrpcIdentityExtractor::extract_claims(request.metadata(),
&self.jwt_validator).await?` → `r2e::StandardClaims` (same typed claims as
HTTP; `Status::unauthenticated` on failure). `JwtClaimsValidator` implements
`JwtClaimsValidatorLike` under `r2e-security/grpc`, enabled by `r2e`'s `grpc` +
`security` features.
