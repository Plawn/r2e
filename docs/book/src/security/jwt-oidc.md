# JWT / OIDC Authentication

R2E provides JWT-based authentication with support for both static keys and JWKS endpoints (for OIDC providers like Keycloak, Auth0, etc.).

## Setup

Enable the security feature:

```toml
r2e = { version = "0.1", features = ["security"] }
```

## Configuring the validator

### Static key (testing / simple setups)

```rust
use r2e::r2e_security::{JwtClaimsValidator, SecurityConfig};
use jsonwebtoken::DecodingKey;

let config = SecurityConfig::new("unused-jwks-url", "my-issuer", "my-audience");
let key = DecodingKey::from_secret(b"my-secret-key");
let validator = JwtClaimsValidator::new_with_static_key(key, config);
```

### JWKS endpoint (production)

```rust
use std::sync::Arc;
use r2e::r2e_security::{JwksCache, JwtClaimsValidator, SecurityConfig};

let config = SecurityConfig::new(
    "https://auth.example.com/.well-known/jwks.json",
    "https://auth.example.com",
    "my-app",
);
let jwks = Arc::new(JwksCache::new(config.clone()).await.unwrap());
let validator = JwtClaimsValidator::new(jwks, config);
```

The JWKS keys are fetched and cached automatically. Cache misses trigger a background refresh.

> Most apps build the validator inside a `#[producer]` and register it with
> `.register::<JwtValidator>()` — the `r2e new --auth` scaffold generates exactly
> this. The snippet above is the equivalent manual construction.

## Providing the validator as a bean

The `AuthenticatedUser` extractor resolves `Arc<JwtClaimsValidator>` from the bean
graph by type. Provide it during app assembly — there is no state struct to write:

```rust
use std::sync::Arc;

AppBuilder::new()
    .provide(Arc::new(validator))   // Arc<JwtClaimsValidator> becomes a bean
    // ... other .provide / .register calls
    .build_state()
    .await
    // ...
```

Any controller with `#[inject(identity)]` (or a handler-param `#[inject(identity)]`)
picks the validator up automatically; if it is missing you get a compile error
naming `Arc<JwtClaimsValidator>`.

## `AuthenticatedUser` extractor

`AuthenticatedUser` is an Axum `FromRequestParts` extractor that validates the JWT bearer token:

```rust
use r2e::r2e_security::AuthenticatedUser;

#[controller(path = "/users")]
pub struct UserController {
    #[inject(identity)] user: AuthenticatedUser,
}

#[routes]
impl UserController {
    #[get("/me")]
    async fn me(&self) -> Json<AuthenticatedUser> {
        Json(self.user.clone())
    }
}
```

### Available fields

| Field | Type | Description |
|-------|------|-------------|
| `sub` | `String` | Unique subject identifier |
| `email` | `Option<String>` | Email address |
| `roles` | `Vec<String>` | Extracted roles |
| `claims` | `serde_json::Value` | Raw JWT claims |

### Utility methods

```rust
user.has_role("admin")           // check single role
user.has_any_role(&["admin", "moderator"])  // check any of roles
```

## Authentication flow

1. Client sends `Authorization: Bearer <token>` header
2. R2E extracts the token
3. Token signature is validated (static key or JWKS lookup)
4. Claims are extracted (`sub`, `email`, `roles`)
5. `AuthenticatedUser` is constructed
6. If validation fails → 401 Unauthorized (handler never executes)

## Role extraction

R2E extracts roles from two locations (checked in order):
1. Top-level `roles` claim: `{"roles": ["admin", "user"]}`
2. Keycloak format: `{"realm_access": {"roles": ["admin", "user"]}}`

This is handled by the `DefaultRoleExtractor`. Custom extraction can be provided by implementing the `RoleExtractor` trait.

## Struct-level vs param-level identity

**Struct-level** — all endpoints require authentication:

```rust
#[controller(path = "/users")]
pub struct UserController {
    #[inject(identity)] user: AuthenticatedUser,
}
```

**Param-level** — only annotated endpoints require authentication:

```rust
#[controller(path = "/api")]
pub struct ApiController {
    #[inject] service: MyService,
}

#[routes]
impl ApiController {
    #[get("/public")]
    async fn public_data(&self) -> Json<Data> { /* no auth */ }

    #[get("/me")]
    async fn me(&self, #[inject(identity)] user: AuthenticatedUser) -> Json<User> {
        Json(user)
    }
}
```

Param-level is more efficient — JWT validation only runs on endpoints that need it.

## Optional identity

For endpoints that work with or without authentication:

```rust
#[get("/greeting")]
async fn greeting(
    &self,
    #[inject(identity)] user: Option<AuthenticatedUser>,
) -> String {
    match user {
        Some(u) => format!("Hello, {}!", u.sub),
        None => "Hello, stranger!".to_string(),
    }
}
```

## Configuration via YAML

```yaml
security:
  jwt:
    issuer: "https://auth.example.com"
    audience: "my-app"
    jwks-url: "https://auth.example.com/.well-known/jwks.json"
```

## Embedded OIDC server

To issue JWT tokens directly from your application — without an external provider — see the [Embedded OIDC Server](./embedded-oidc.md).
