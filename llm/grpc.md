---
topic: grpc
features: grpc
tokens: ~2600
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
- Identity uses the HTTP extractor contract: a struct-level
  `#[inject(identity)] user: AuthenticatedUser` field (every method
  authenticated) or an `#[inject(identity)]` method **parameter** (or
  `Option<..>`), read from `authorization: Bearer ..` metadata and validated
  by the `Arc<JwtClaimsValidator>` bean — provide it, or
  `register_grpc_service` fails to compile. `#[inject(request)]` fields work
  too. `#[roles]` needs a struct or parameter identity.
- Not supported on `#[grpc_routes]` (compile errors): `#[pre_guard]`,
  `#[anonymous]` (use `Option<I>`), `#[post_construct]`, `#[pre_destroy]`,
  `#[on_start]`, `Self` in a method signature.
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
use r2e::http::layers::cors::CorsLayer;

# fn __doc(b: AppBuilder) -> impl Sized {
b.plugin(GrpcServer::multiplexed().with_grpc_web())                  // default CORS: any origin, POST/OPTIONS, grpc-status/-message exposed
# }
# fn __doc2(b: AppBuilder) -> impl Sized {
b.plugin(GrpcServer::multiplexed().with_grpc_web_cors(CorsLayer::new().allow_methods([Method::POST, Method::OPTIONS])))  // your own CorsLayer (tower-http's type, re-exported)
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
`register_grpc_service`).

Each call is turned into an HTTP request head (`http::request::Parts`):
`method` is `POST`, `uri` is `/<package.Service>/<Method>`, `headers` are the
request metadata (moved, not copied), the tonic extensions are kept and
`ConnectInfo` carries the peer address (unless a layer already inserted one).
No `MetadataMap` is put in the extensions: an extractor that wants the tonic
view builds `MetadataMap::from_headers(parts.headers.clone())` itself. Identity and `#[inject(request)]` values are extracted from that
head through `FromRequestPartsVia<S, M>` — the same contract as HTTP — so any
HTTP identity or extractor works unchanged, and the guards receive a
`GuardContext` built over it (`path_params` empty).

Identity is either a struct-level `#[inject(identity)]` field — every method
is authenticated, fail-closed, and runs on the per-call façade — or an
`#[inject(identity)]` **method parameter** (any position). `Option<T>` works in
both places: `None` without `authorization` metadata, still rejecting an
invalid token. `AuthenticatedUser` validates `authorization: Bearer <jwt>`
with the `Arc<JwtClaimsValidator>` bean, a compile-time dependency of the
service (checked through the `GrpcService<S, W>` witness).

```rust
use r2e::r2e_grpc::tonic::{Request, Response, Status};
use r2e::r2e_security::AuthenticatedUser;

#[controller]
pub struct SecureGreeter {
    #[inject] user_service: UserService,
    #[inject(identity)] user: AuthenticatedUser,   // every method authenticated
}

#[grpc_routes(proto::greeter::greeter_server::Greeter)]
#[intercept(Logged::info())]
impl SecureGreeter {
    #[roles("admin")]
    async fn say_hello(&self, request: Request<HelloRequest>) -> Result<Response<HelloReply>, Status> {
        Ok(Response::new(HelloReply { message: format!("Hello {} from {}", request.into_inner().name, self.user.sub) }))
    }
}

#[controller]
pub struct MixedGreeter {
    #[inject] user_service: UserService,
}

#[grpc_routes(proto::greeter::greeter_server::Greeter)]
impl MixedGreeter {
    async fn say_hello(
        &self,
        #[inject(identity)] user: Option<AuthenticatedUser>,   // per-method, optional
        request: Request<HelloRequest>,
    ) -> Result<Response<HelloReply>, Status> {
        let who = user.map(|u| u.sub).unwrap_or_else(|| "anonymous".into());
        Ok(Response::new(HelloReply { message: format!("Hello {} from {who}", request.into_inner().name) }))
    }
}
# fn main() {}
```

Server-streaming rpcs work: declare the trait's associated stream type in the
impl block (`type WatchStream = Pin<Box<dyn Stream<Item = Result<Tick, Status>> + Send>>;`);
identity, request scope and guards all run before the stream is opened.

Per call: head → struct request scope (identity + `#[inject(request)]`
fields) → identity parameter → controller guards → method guards →
interceptors → method. Every `Rejection` is projected onto `tonic::Status` by
kind with `r2e_grpc::rejection_to_status` (Unauthenticated → `UNAUTHENTICATED`,
Forbidden → `PERMISSION_DENIED`, NotFound → `NOT_FOUND`, Conflict → `ABORTED`,
RateLimited/PayloadTooLarge → `RESOURCE_EXHAUSTED`, Unavailable →
`UNAVAILABLE`, Timeout → `DEADLINE_EXCEEDED`, Internal → `INTERNAL`,
request-shape kinds → `INVALID_ARGUMENT`, else by HTTP status); the message is
the status message and the rejection's headers (`Retry-After`,
`WWW-Authenticate`) become response metadata. It is a free function (orphan
rule) — call it yourself only when projecting a `Rejection` by hand.

Compile errors: `#[roles]` on a method of a service with neither a
struct-level identity nor an `#[inject(identity)]` parameter
(`NoIdentity: RoleBasedIdentity` unsatisfied); a guard whose spec has
`REQUIRES_IDENTITY` on such a method; a missing extractor bean at
`register_grpc_service`; `#[pre_guard]`, `#[anonymous]`, `#[post_construct]`,
`#[pre_destroy]`, `#[on_start]`.

An HTTP-only extractor (`Query<T>`, `Path<T>`) compiles and its outcome
depends on the head and the target type — `Query<T>` deserializes an empty
query string (a `T` whose fields are all optional/defaulted succeeds, a
required field is an `INVALID_ARGUMENT`), `Path<T>` sees the `{*rest}`
capture tonic's `Routes` installs — never a panic; a
rejection becomes a `Status` by kind. Message of a missing/invalid bearer:
`UNAUTHENTICATED` / `"Unauthorized"` (the HTTP extractor's), without
`www-authenticate` metadata.

Manual claims: `r2e_grpc::bearer_token(metadata) -> Result<&str, Rejection>`,
`extract_jwt_claims_from_metadata(metadata, &validator).await ->
Result<StandardClaims, Status>`.
