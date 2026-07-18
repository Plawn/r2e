# Guards and Identity

> **Status:** The guard and identity infrastructure is implemented in `r2e-grpc` as runtime types, but **not yet wired into the `#[grpc_routes]` macro**. Currently, only `#[intercept]` is supported as a method decorator. `#[roles]`, `#[guard]`, and `#[inject(identity)]` will be enabled in a future release.

## Manual identity extraction (available now)

While `#[inject(identity)]` is not yet available as a macro decorator, you can extract identity manually using the runtime functions:

```rust
use r2e::r2e_grpc::{extract_bearer_token, GrpcIdentityExtractor};

#[grpc_routes(proto::greeter::greeter_server::Greeter)]
impl GreeterService {
    async fn say_hello(
        &self,
        request: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        let metadata = request.metadata();
        let claims = GrpcIdentityExtractor::extract_claims(metadata, &self.jwt_validator).await?;
        let sub = claims["sub"].as_str().unwrap_or("unknown");

        Ok(tonic::Response::new(HelloReply {
            message: format!("Hello {}!", sub),
        }))
    }
}
```

This requires the controller to have `#[inject] jwt_validator: Arc<JwtClaimsValidator>`.

## Runtime types (available now)

The following types are exported by `r2e-grpc` and ready for use:

| Type | Description |
|------|-------------|
| `GrpcGuard<I>` | Guard trait for gRPC methods (analog of `Guard<I>` for HTTP) |
| `GrpcGuardContext<'a, I>` | Context passed to guards (service name, method name, metadata, identity) |
| `GrpcRolesGuard` | Built-in guard that checks required roles |
| `GrpcRoleBasedIdentity` | Extension trait for identity types that carry roles |
| `GrpcIdentityExtractor` | JWT extraction from gRPC metadata |
| `JwtClaimsValidatorLike` | Trait abstracting JWT validation (blanket impl for `Arc<T>`) |

### Guard context

`GrpcGuardContext` provides access to service metadata and identity:

| Field/Method | Type | Description |
|---|---|---|
| `service_name` | `&'static str` | Proto service name |
| `method_name` | `&'static str` | RPC method name |
| `metadata` | `&MetadataMap` | gRPC request metadata |
| `identity` | `Option<&I>` | Authenticated identity (if extracted) |
| `identity_sub()` | `Option<&str>` | Subject from identity |
| `identity_email()` | `Option<&str>` | Email from identity |
| `identity_claims()` | `Option<&Value>` | Raw JWT claims |

---

The following sections document the **planned API** for when macro support is enabled. The runtime types above already support these patterns — only the macro integration is pending.

---

## Identity extraction (planned)

`#[inject(identity)]` on method parameters will extract authenticated identity from gRPC metadata:

```rust
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

Identity is extracted from the `authorization` metadata key using the `Bearer` scheme, then validated with `JwtClaimsValidator` — the same validator used for HTTP requests.

### Optional identity (planned)

Use `Option<AuthenticatedUser>` for methods that work with or without authentication:

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

### How extraction works

The extraction pipeline (implemented in `r2e_grpc::identity`):

1. Read `authorization` metadata from `tonic::Request`
2. Strip the `Bearer ` prefix (supports `Bearer` and `bearer`)
3. Validate the token with `JwtClaimsValidator` (the `Arc<JwtClaimsValidator>` bean read from the resolved graph)
4. Build `AuthenticatedUser` from the validated claims

If validation fails, the method returns `Status::unauthenticated` before the handler body runs.

## Role-based guards (planned)

`#[roles("...")]` will restrict methods to specific roles:

```rust
#[grpc_routes(proto::greeter::greeter_server::Greeter)]
impl GreeterService {
    #[roles("admin")]
    async fn say_hello_admin(
        &self,
        request: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        // Only reachable if the caller has the "admin" role
        Ok(tonic::Response::new(HelloReply {
            message: format!("[ADMIN] Hello {}!", request.get_ref().name),
        }))
    }
}
```

The built-in `GrpcRolesGuard` checks roles via the `GrpcRoleBasedIdentity` trait. If the caller lacks the required role, the guard returns `Status::permission_denied("Insufficient roles")`.

## Custom guards (planned)

Implement the `GrpcGuard` trait for custom authorization logic:

```rust
use std::future::Future;
use r2e::r2e_grpc::{GrpcGuard, GrpcGuardContext};
use r2e::Identity;
use tonic::Status;

struct TenantGuard;

impl<I: Identity> GrpcGuard<I> for TenantGuard {
    fn check(
        &self,
        ctx: &GrpcGuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Status>> + Send {
        async move {
            let tenant = ctx.metadata
                .get("x-tenant-id")
                .and_then(|v| v.to_str().ok());

            match tenant {
                Some(_) => Ok(()),
                None => Err(Status::permission_denied("Missing tenant ID")),
            }
        }
    }
}
```

Once enabled, apply with `#[guard(...)]`:

```rust
#[guard(TenantGuard)]
async fn create_user(
    &self,
    request: tonic::Request<CreateUserRequest>,
) -> Result<tonic::Response<UserResponse>, tonic::Status> {
    // ...
}
```

### Guards that need beans (planned)

Unlike HTTP guards, gRPC guards do **not** go through `DecoratorSpec` — they are
plain `GrpcGuard<I>` implementations. A guard that needs a database pool (or any
service) holds it as a **field**, constructed by the caller who wires the guard
onto the service:

```rust
struct ActiveUserGuard {
    pool: SqlitePool,   // resolved by the caller, held as a field
}

impl<I: Identity> GrpcGuard<I> for ActiveUserGuard {
    fn check(
        &self,
        ctx: &GrpcGuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Status>> + Send {
        async move {
            let sub = ctx.identity_sub().unwrap_or("");

            let active = sqlx::query_scalar::<_, bool>(
                "SELECT active FROM users WHERE sub = ?"
            )
            .bind(sub)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| Status::internal("Database error"))?;

            match active {
                Some(true) => Ok(()),
                _ => Err(Status::permission_denied("Account suspended")),
            }
        }
    }
}
```

### Combining guards (planned)

Guards will be stackable and execute in order:

```rust
#[roles("editor")]
#[guard(TenantGuard)]
#[guard(ActiveUserGuard)]
async fn update_user(
    &self,
    request: tonic::Request<UpdateUserRequest>,
) -> Result<tonic::Response<UserResponse>, tonic::Status> {
    // Reached only if all guards pass
}
```

Execution order: roles check first, then custom guards in declaration order. Short-circuits on first failure.

## HTTP vs gRPC comparison

| | HTTP | gRPC |
|-|------|------|
| Guard trait | `Guard<I>` | `GrpcGuard<I>` |
| Error type | `Response` (HTTP response) | `tonic::Status` |
| Context type | `GuardContext` | `GrpcGuardContext` |
| Request metadata | `&HeaderMap` + `&Uri` | `&MetadataMap` |
| Role guard | `RolesGuard` | `GrpcRolesGuard` |
| Attribute | `#[guard(...)]` | `#[guard(...)]` (same syntax) |

The concepts are identical — only the error type and metadata source differ.

## Setup for identity

To use identity extraction in gRPC services, `Arc<JwtClaimsValidator>` must be a bean in the graph (same requirement as HTTP) — provide it before `build_state()`:

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

There is no hand-written state struct: the application state is the inferred HList of everything you `.provide()`/`.register()`, and beans are read back by type. The gRPC identity extractor uses `JwtClaimsValidatorLike`, a trait that `Arc<JwtClaimsValidator>` implements automatically via a blanket impl. No additional setup needed beyond what HTTP authentication already requires.

### GrpcRoleBasedIdentity

For role-based guards, your identity type must implement `GrpcRoleBasedIdentity`:

```rust
use r2e::r2e_grpc::GrpcRoleBasedIdentity;

impl GrpcRoleBasedIdentity for AuthenticatedUser {
    fn roles(&self) -> &[String] {
        &self.roles
    }
}
```

`AuthenticatedUser` from `r2e-security` already has role information — if you're using it, this is the only additional trait needed.

## Limitations

- **No pre-auth guards** — gRPC doesn't have the same pre-auth/post-auth distinction as HTTP. All guards run after identity extraction is attempted.
- **No struct-level identity** — gRPC services cannot have `#[inject(identity)]` on struct fields (the core is built from the bean context via `ContextConstruct`, with no request to extract from). Use param-level injection instead.
- **Metadata vs headers** — gRPC uses `tonic::metadata::MetadataMap`, not HTTP `HeaderMap`. Custom guards must use the metadata API.

## Next steps

- [gRPC Services](./services.md) — setup and service implementation
- [Custom Guards](../advanced/custom-guards.md) — HTTP guard patterns (same concepts apply)
- [JWT / OIDC Authentication](../security/jwt-oidc.md) — JWT validator setup
