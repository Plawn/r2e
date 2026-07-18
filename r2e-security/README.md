# r2e-security

JWT/OIDC security module for R2E — token validation, JWKS cache, and `AuthenticatedUser` extractor.

## Overview

Provides JWT-based authentication with support for static keys (testing/development) and JWKS endpoints (production with OIDC providers like Keycloak, Auth0, etc.).

## Usage

Via the facade crate (enabled by default):

```toml
[dependencies]
r2e = "0.1"  # security is a default feature
```

## Key types

### AuthenticatedUser

Axum extractor that validates Bearer tokens and exposes user identity:

```rust
use r2e::r2e_security::AuthenticatedUser;

#[controller(path = "/users")]
pub struct UserController {
    #[inject(identity)] user: AuthenticatedUser,
}

#[routes]
impl UserController {
    #[get("/me")]
    async fn me(&self) -> Json<String> {
        Json(self.user.sub.clone())
    }
}
```

`AuthenticatedUser` implements the `Identity` trait, providing:
- `sub()` — unique subject identifier
- `email()` — email address (optional)
- `claims()` — raw JWT claims (optional)

Roles live on the separate `RoleBasedIdentity` trait (`roles() -> &[String]`),
which `AuthenticatedUser` also implements. That trait is what `#[roles(...)]` /
`#[all_roles(...)]` require of an identity type.

The extractor validates the token with an `Arc<JwtClaimsValidator>` **resolved
from the bean graph by type** — so you must provide one as a bean before
`build_state()`:

```rust
AppBuilder::new()
    .provide(std::sync::Arc::new(claims_validator))  // Arc<JwtClaimsValidator>
    // ...
    .build_state().await
```

`AuthenticatedUser` extracts via `FromRequestPartsVia` (its `HasBean<Arc<JwtClaimsValidator>, _>`
witness is inferred at the call site); no `state = ...` and no `FromRef` bound is involved.

### JwtValidator

Higher-level validator with builder pattern:

```rust
use std::sync::Arc;
use r2e::r2e_security::{JwksCache, JwtClaimsValidator, SecurityConfig};

// Production: JWKS endpoint
let config = SecurityConfig::new(
    "https://auth.example.com/.well-known/jwks.json",
    "https://auth.example.com",
    "my-api",
);

let jwks = Arc::new(JwksCache::new(config.clone()).await?);
let validator = JwtClaimsValidator::new(jwks, config);
```

JWKS requests are HTTPS-only by default, including redirects. Cached keys have
a one-hour TTL and may be used for one additional hour during an upstream
outage; configure that bounded grace period with `with_max_stale()`.

### Role-based access control

```rust
#[routes]
impl AdminController {
    #[get("/dashboard")]
    #[roles("admin")]
    async fn dashboard(&self) -> &'static str {
        "Admin only"
    }
}
```

### RoleExtractor

Trait-based role extraction to support multiple OIDC providers. The default `DefaultRoleExtractor` checks top-level `roles` and Keycloak's `realm_access.roles`.

### Parameter-level identity

For mixed controllers with both public and protected endpoints:

```rust
#[controller(path = "/api")]
pub struct MixedController {
    #[inject] service: MyService,
}

#[routes]
impl MixedController {
    #[get("/public")]
    async fn public_data(&self) -> Json<Vec<Data>> { ... }

    #[get("/me")]
    async fn me(&self, #[inject(identity)] user: AuthenticatedUser) -> Json<String> {
        Json(user.sub.clone())
    }
}
```

## License

Apache-2.0
