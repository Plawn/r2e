---
topic: grpc
features: grpc
tokens: ~2200
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
- Guards are the HTTP ones: `#[guard(..)]`, `#[roles(..)]`, `#[all_roles(..)]`
  and `#[intercept(..)]` on methods or on the impl block; a `Guard<I>` written
  for HTTP runs unchanged. A guard or identity `Rejection` is projected onto
  `tonic::Status` by kind (`r2e_grpc::rejection_to_status`: Unauthenticated →
  `UNAUTHENTICATED`, Forbidden → `PERMISSION_DENIED`, RateLimited →
  `RESOURCE_EXHAUSTED`, request-shape kinds → `INVALID_ARGUMENT`, …).
- Identity is a method **parameter**: `#[inject(identity)] user:
  AuthenticatedUser` (or `Option<..>`), read from `authorization: Bearer ..`
  metadata and validated by the `Arc<JwtClaimsValidator>` bean — provide it,
  or `register_grpc_service` fails to compile. `#[roles]` needs that parameter.
- Not supported on `#[grpc_routes]` (compile errors): struct-level
  `#[inject(identity)]`, `#[pre_guard]`, `#[anonymous]`, `#[post_construct]`,
  `#[pre_destroy]`, `#[on_start]`.
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

### Guards and identity

gRPC methods take the same decorators as HTTP routes — `#[guard]`, `#[roles]`,
`#[all_roles]`, `#[intercept]` — on the method or on the impl block
(controller-level sites run first). Guards are `Guard<I>` impls, built once at
registration through `DecoratorSpec` (bean deps checked at
`register_grpc_service`), and receive a `GuardContext` whose `headers` is the
request metadata (`method` is `POST`, `uri` is `/`, `path_params` empty).

Identity is an `#[inject(identity)]` **method parameter** (any position;
`Option<T>` for optional). The type must implement `r2e_grpc::GrpcIdentity`;
`AuthenticatedUser` does (features `grpc` + `security`): `authorization:
Bearer <jwt>` metadata validated by the `Arc<JwtClaimsValidator>` bean, which
becomes a compile-time dependency of the service. Optional identity is `None`
without `authorization` metadata but still rejects an invalid token.

```rust
use r2e::r2e_grpc::tonic::{Request, Response, Status};
use r2e::r2e_security::AuthenticatedUser;

#[controller]
pub struct SecureGreeter {
    #[inject] user_service: UserService,
}

#[grpc_routes(proto::greeter::greeter_server::Greeter)]
#[intercept(Logged::info())]
impl SecureGreeter {
    #[roles("admin")]
    async fn say_hello(
        &self,
        #[inject(identity)] user: AuthenticatedUser,
        request: Request<HelloRequest>,
    ) -> Result<Response<HelloReply>, Status> {
        Ok(Response::new(HelloReply { message: format!("Hello {} from {}", request.into_inner().name, user.sub) }))
    }
}
# fn main() {}
```

Per call: identity extraction → controller guards → method guards →
interceptors → method. Every `Rejection` is projected onto `tonic::Status` by
kind with `r2e_grpc::rejection_to_status` (Unauthenticated → `UNAUTHENTICATED`,
Forbidden → `PERMISSION_DENIED`, NotFound → `NOT_FOUND`, Conflict → `ABORTED`,
RateLimited/PayloadTooLarge → `RESOURCE_EXHAUSTED`, Unavailable →
`UNAVAILABLE`, Timeout → `DEADLINE_EXCEEDED`, Internal → `INTERNAL`,
request-shape kinds → `INVALID_ARGUMENT`, else by HTTP status); the message is
the status message and the rejection's headers (`Retry-After`,
`WWW-Authenticate`) become response metadata. It is a free function (orphan
rule) — call it yourself only when projecting a `Rejection` by hand.

Compile errors: `#[roles]` on a method without an `#[inject(identity)]`
parameter (`NoIdentity: RoleBasedIdentity` unsatisfied); a guard whose spec
has `REQUIRES_IDENTITY` on such a method; struct-level `#[inject(identity)]`;
`#[pre_guard]`, `#[anonymous]`, `#[post_construct]`, `#[pre_destroy]`,
`#[on_start]`.

Manual claims: `r2e_grpc::bearer_token(metadata) -> Result<&str, Rejection>`,
`extract_jwt_claims_from_metadata(metadata, &validator).await ->
Result<StandardClaims, Status>`.
