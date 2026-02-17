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

#[derive(Controller)]
#[controller(path = "/users", state = AppState)]
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
- `roles()` — role list
- `email()` — email address (optional)
- `claims()` — raw JWT claims

### JwtValidator

Higher-level validator with builder pattern:

```rust
use r2e::r2e_security::{JwtValidator, SecurityConfig};

// Production: JWKS endpoint
let config = SecurityConfig::new("https://auth.example.com/.well-known/jwks.json")
    .with_issuer("https://auth.example.com")
    .with_audience("my-api");

let validator = JwtValidator::from_config(config).await?;
```

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
#[derive(Controller)]
#[controller(path = "/api", state = AppState)]
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
