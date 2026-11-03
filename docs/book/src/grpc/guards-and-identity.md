# Guards and Identity

gRPC methods use the **same** guard model as HTTP routes: `Guard<I>`,
`#[guard(...)]`, `#[roles(...)]`, `#[all_roles(...)]` and `#[inject(identity)]`
fields and parameters. A guard written for HTTP runs on a gRPC method
unchanged, and so does an identity extractor: every call is turned into an
HTTP request head and the identity is extracted from it through the HTTP
contract (`FromRequestPartsVia`). The only transport-specific piece is *how* a
`Rejection` becomes a `tonic::Status`.

## Identity extraction

### Struct-level identity

Declare the identity on the service struct, like an HTTP controller. Every
method is authenticated (fail-closed) and reads it through `self`:

```rust
use r2e::r2e_security::AuthenticatedUser;

#[controller]
pub struct AccountService {
    #[inject(identity)]
    user: AuthenticatedUser,
}

#[grpc_routes(proto::accounts::accounts_server::Accounts)]
impl AccountService {
    #[roles("admin")]
    async fn close(
        &self,
        request: tonic::Request<CloseRequest>,
    ) -> Result<tonic::Response<CloseReply>, tonic::Status> {
        tracing::info!(by = %self.user.sub, "closing account");
        // ...
    }
}
```

The service core is still built once from the bean graph; per call the
identity (and any `#[inject(request)]` field — `HeaderMap`, `Uri`,
`ConnectInfo<SocketAddr>`, …) is extracted and the method runs on a per-call
façade over the core, exactly as on HTTP. Because the method runs on the
façade, `Self` is not allowed in a method signature (`Self::<Assoc>` paths
are rewritten to the declared type), and a helper called on `self` — sync or
`async` — needs `#[request_helper]`.

### Parameter identity

Or add an `#[inject(identity)]` parameter to the method — the identity is
extracted before the handler body runs:

```rust
use r2e::r2e_security::AuthenticatedUser;

#[grpc_routes(proto::greeter::greeter_server::Greeter)]
impl GreeterService {
    async fn say_hello(
        &self,
        request: tonic::Request<HelloRequest>,
        #[inject(identity)] user: AuthenticatedUser,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        let reply = HelloReply {
            message: format!("Hello {}, you are {}!", request.get_ref().name, user.sub),
        };
        Ok(tonic::Response::new(reply))
    }
}
```

The identity parameter can sit before or after the request parameter — the
generated tonic method keeps the trait's signature and passes both to yours.

Identity is read from the `authorization` metadata key using the `Bearer`
scheme, then validated with `JwtClaimsValidator` — the same validator bean and
the same extractor HTTP uses. A missing or invalid token answers
`UNAUTHENTICATED` (message `Unauthorized`) before any guard runs.

### Optional identity

Use `Option<AuthenticatedUser>` for methods that work with or without
authentication:

```rust
async fn say_hello(
    &self,
    request: tonic::Request<HelloRequest>,
    #[inject(identity)] user: Option<AuthenticatedUser>,
) -> Result<tonic::Response<HelloReply>, tonic::Status> {
    let greeting = match &user {
        Some(u) => format!("Hello {}!", u.sub),
        None => "Hello anonymous!".to_string(),
    };
    Ok(tonic::Response::new(HelloReply { message: greeting }))
}
```

No `authorization` metadata yields `None`. A token that *is* present but
invalid is still rejected with `UNAUTHENTICATED` — optional means "anonymous is
fine", not "garbage is fine".

### How extraction works

The generated tonic method turns the call into an `http::request::Parts`:

| Head field | From the gRPC call |
|---|---|
| `method` | `POST` |
| `uri` | `/<package.Service>/<Method>` (recorded by the `CaptureMethodPath` routing layer) |
| `headers` | the request metadata, moved without copying |
| `extensions` | the tonic request extensions, moved, plus `ConnectInfo<SocketAddr>` from the peer address |

The identity type is then extracted exactly as on an HTTP route, through
`FromRequestPartsVia<S, M>` against the application state. `AuthenticatedUser`
implements it with the `Arc<JwtClaimsValidator>` bean (`ViaBean`). A custom
identity type implements `FromRequestParts` or `FromRequestPartsVia` once and
works on HTTP routes, MCP members and gRPC methods. After extraction the head
is put back around the message, so the method receives a normal
`tonic::Request` whose metadata and extensions reflect what the extractors
left.

The validator is a **compile-time dependency** of the service: the generated
`GrpcService<S, W>` impl only exists for a state the extractor resolves
against, so forgetting to `.provide()` the `Arc<JwtClaimsValidator>` bean fails
at `register_grpc_service::<S>()` with "bean `Arc<JwtClaimsValidator>` is not
present in the application state".

Migrating from the pre-0.10 gRPC identity trait? See `docs/migration/grpc-identity.md`.

## Role-based guards

`#[roles("...")]` restricts a method to callers holding one of the roles;
`#[all_roles("...")]` requires all of them:

```rust
#[grpc_routes(proto::greeter::greeter_server::Greeter)]
impl GreeterService {
    #[roles("admin")]
    async fn say_hello_admin(
        &self,
        #[inject(identity)] user: AuthenticatedUser,
        request: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        // Only reachable if the caller has the "admin" role
        Ok(tonic::Response::new(HelloReply {
            message: format!("[ADMIN] Hello {}!", request.get_ref().name),
        }))
    }
}
```

This is the HTTP `RolesGuard` — it needs an identity to read roles from, so a
`#[roles]` method needs a struct-level identity or an `#[inject(identity)]`
parameter. Without either the identity type is `NoIdentity`, which does not implement
`RoleBasedIdentity`, and the build fails with
"the trait bound `NoIdentity: RoleBasedIdentity` is not satisfied". A caller
lacking the role gets `PERMISSION_DENIED` ("Insufficient roles").

## Custom guards

Implement `Guard<I>` — the HTTP guard trait — and apply it with `#[guard(...)]`.
The context is a `GuardContext` built over the call head
(`GuardContext::from_parts`): `headers` are the metadata, `uri` is the method
path, `extensions` and `peer_addr` come from the tonic request.

```rust
use std::future::Future;
use r2e::{Guard, GuardContext, Identity, Rejection, SelfBuilt};

pub struct TenantGuard;

impl SelfBuilt for TenantGuard {}

impl<I: Identity> Guard<I> for TenantGuard {
    fn check(
        &self,
        ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Rejection>> + Send {
        let has_tenant = ctx.headers.contains_key("x-tenant-id");
        async move {
            if has_tenant {
                Ok(())
            } else {
                Err(Rejection::forbidden("Missing tenant ID"))
            }
        }
    }
}
```

```rust
#[guard(TenantGuard)]
async fn create_user(
    &self,
    request: tonic::Request<CreateUserRequest>,
) -> Result<tonic::Response<UserResponse>, tonic::Status> {
    // ...
}
```

Guards go through `DecoratorSpec` exactly as on HTTP: a guard that needs a
bean uses `#[derive(DecoratorBean)]`, and the dependency is checked at
`register_grpc_service`. See [Custom Guards](../advanced/custom-guards.md) —
every pattern there applies verbatim.

A guard whose spec declares `REQUIRES_IDENTITY` (it only makes sense with a
`Some` identity) on a method of a service with neither a struct-level identity
nor an `#[inject(identity)]` parameter is a compile error: the guard could
never pass.

### Controller-level guards

`#[guard]`, `#[roles]` and `#[all_roles]` on the `#[grpc_routes]` impl block
apply to every method, before the method's own guards:

```rust
#[grpc_routes(proto::greeter::greeter_server::Greeter)]
#[guard(TenantGuard)]
impl GreeterService {
    // every method checks TenantGuard first
}
```

### Combining guards

Guards stack and run in order:

```rust
#[roles("editor")]
#[guard(TenantGuard)]
#[guard(ActiveUserGuard)]
async fn update_user(
    &self,
    #[inject(identity)] user: AuthenticatedUser,
    request: tonic::Request<UpdateUserRequest>,
) -> Result<tonic::Response<UserResponse>, tonic::Status> {
    // Reached only if all guards pass
}
```

Per call: head → struct request scope → identity parameter → controller
guards → method guards in declaration order → interceptors → your method.
For a server-streaming rpc all of it runs before the stream opens. Short-circuits on the first
failure.

## From `Rejection` to `tonic::Status`

Guards and identity return a `Rejection` (see
[Error Handling](../core-concepts/error-handling.md)). On gRPC it is projected by
**kind** with `r2e_grpc::rejection_to_status`:

| `RejectionKind` | `tonic::Code` |
|---|---|
| `Unauthenticated` | `UNAUTHENTICATED` |
| `Forbidden` | `PERMISSION_DENIED` |
| `NotFound` | `NOT_FOUND` |
| `Conflict` | `ABORTED` |
| `RateLimited`, `PayloadTooLarge` | `RESOURCE_EXHAUSTED` |
| `Unavailable` | `UNAVAILABLE` |
| `Timeout` | `DEADLINE_EXCEEDED` |
| `Internal` | `INTERNAL` |
| request-shape kinds (`BadRequest`, `Validation`, `Invalid*`, `MalformedBody`, …) | `INVALID_ARGUMENT` |
| anything else (incl. `Opaque`) | by HTTP status (`code_from_status`): 4xx → `INVALID_ARGUMENT`, 5xx → `INTERNAL` |

The rejection's message becomes the status message (an empty message falls
back to `request rejected with status N`), and its headers (`Retry-After`,
`WWW-Authenticate`, …) become response metadata. Because `tonic::Status` is a
foreign type, this is a free function rather than a `From` impl; the generated
code calls it for you — you only need it when projecting a `Rejection` by hand
inside a method.

## Setup for identity

`Arc<JwtClaimsValidator>` must be a bean in the graph — the same requirement as
HTTP identity. Provide it before `build_state()`:

```rust
use std::sync::Arc;
use r2e::r2e_security::JwtClaimsValidator;

AppBuilder::new()
    .plugin(GrpcServer::on_port("0.0.0.0:50051"))
    .provide(Arc::new(jwt_validator))   // Arc<JwtClaimsValidator> as a bean
    .build_state()
    .await
    .register_grpc_service::<GreeterService>();
```

There is no hand-written state struct: the application state is the inferred
HList of everything you `.provide()`/`.register()`, and the service's
`GrpcService<S, W>` impl requires the validator whenever it injects an
`AuthenticatedUser`. Nothing beyond what HTTP authentication already requires.

### Manual extraction

Inside a method you can still read claims by hand — for a custom identity type,
or a method that only needs one claim:

```rust
use r2e::r2e_grpc::{bearer_token, extract_jwt_claims_from_metadata, rejection_to_status};

let token = bearer_token(request.metadata()).map_err(rejection_to_status)?;
let claims = extract_jwt_claims_from_metadata(request.metadata(), &self.jwt_validator).await?;
```

`extract_bearer_token` is the same read already projected to a `tonic::Status`.

## Limitations

- **No pre-auth guards** — `#[pre_guard]` is rejected on `#[grpc_routes]`;
  every gRPC guard runs after identity extraction has been attempted.
- **No `#[anonymous]`** — a required struct identity covers every method; a
  service mixing public and protected methods uses `Option<I>` (field or
  parameter).
- **No path parameters** — `method` is always `POST`, `uri` is the method path,
  `path_params` is empty. HTTP-only extractors (`Query<T>`, `Path<T>`) compile
  but fail per call as a `Status` (`INVALID_ARGUMENT`), never a panic.
- **No `www-authenticate` metadata** on an identity rejection — the
  `AuthenticatedUser` extractor rejects with an `HttpError`, which carries no
  challenge header (same as HTTP today).

## Next steps

- [gRPC Services](./services.md) — setup and service implementation
- [Custom Guards](../advanced/custom-guards.md) — the shared guard model
- [JWT / OIDC Authentication](../security/jwt-oidc.md) — JWT validator setup
