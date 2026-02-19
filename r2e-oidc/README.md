# r2e-oidc

Embedded OIDC server plugin for R2E — issue JWT tokens without an external identity provider.

## Overview

Provides an OAuth 2.0 / OpenID Connect server that runs inside your application. It generates RSA-2048 keys, exposes standard OIDC endpoints, and automatically provides `Arc<JwtClaimsValidator>` to the bean graph so `AuthenticatedUser` works out-of-the-box.

Ideal for development, testing, prototyping, and monolithic applications that don't need an external IdP.

## Usage

Via the facade crate:

```toml
[dependencies]
r2e = { version = "0.1", features = ["security", "oidc"] }
```

## Setup

Install `OidcServer` as a pre-state plugin with a user store:

```rust
use r2e::prelude::*;
use r2e::r2e_oidc::{OidcServer, InMemoryUserStore, OidcUser};

let users = InMemoryUserStore::new()
    .add_user("alice", "password123", OidcUser {
        sub: "user-1".into(),
        email: Some("alice@example.com".into()),
        roles: vec!["admin".into()],
        ..Default::default()
    });

let oidc = OidcServer::new()
    .with_user_store(users);

AppBuilder::new()
    .plugin(oidc)
    .build_state::<AppState, _>().await
    .register_controller::<UserController>()
    .serve("0.0.0.0:3000").await.unwrap();
```

No manual `JwtClaimsValidator` setup required — the plugin provides it automatically.

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/oauth/token` | Token issuance (password / client_credentials grant) |
| `GET` | `/.well-known/openid-configuration` | OpenID Connect discovery document |
| `GET` | `/.well-known/jwks.json` | Public key in JWKS format |
| `GET` | `/userinfo` | User info (requires Bearer token) |

## Configuration

```rust
OidcServer::new()
    .issuer("https://myapp.example.com")   // JWT `iss` claim (default: "http://localhost:3000")
    .audience("my-app")                     // JWT `aud` claim (default: "r2e-app")
    .token_ttl(7200)                        // Token TTL in seconds (default: 3600)
    .base_path("/auth")                     // Endpoint prefix (default: "")
    .with_user_store(users)
```

With `base_path("/auth")`, endpoints become `/auth/oauth/token`, `/auth/.well-known/openid-configuration`, etc.

## User store

### InMemoryUserStore

Built-in store for development and testing. Passwords are hashed with Argon2:

```rust
let users = InMemoryUserStore::new()
    .add_user("alice", "password123", OidcUser {
        sub: "user-1".into(),
        email: Some("alice@example.com".into()),
        roles: vec!["admin".into()],
        extra_claims: HashMap::from([("tenant_id".into(), json!("t-42"))]),
    });
```

### Custom user store

Implement the `UserStore` trait for your own backend (SQLx, Redis, LDAP, etc.):

```rust
use r2e::r2e_oidc::{UserStore, OidcUser};

struct SqlxUserStore { pool: SqlitePool }

impl UserStore for SqlxUserStore {
    async fn find_by_username(&self, username: &str) -> Option<OidcUser> { /* ... */ }
    async fn verify_password(&self, username: &str, password: &str) -> bool { /* ... */ }
    async fn find_by_sub(&self, sub: &str) -> Option<OidcUser> { /* ... */ }
}
```

## Client credentials grant

For service-to-service communication, register OAuth clients:

```rust
use r2e::r2e_oidc::ClientRegistry;

let clients = ClientRegistry::new()
    .add_client("my-service", "service-secret");

let oidc = OidcServer::new()
    .with_user_store(users)
    .with_client_registry(clients);
```

```bash
curl -X POST http://localhost:3000/oauth/token \
  -d "grant_type=client_credentials" \
  -d "client_id=my-service" \
  -d "client_secret=service-secret"
```

## JWT claims

Tokens are signed with RS256 and include:

| Claim | Source |
|-------|--------|
| `sub` | `OidcUser.sub` or `client_id` |
| `iss` | Configuration |
| `aud` | Configuration |
| `iat` / `exp` | Automatic |
| `roles` | `OidcUser.roles` |
| `email` | `OidcUser.email` (if set) |
| *custom* | `OidcUser.extra_claims` |

Reserved claims (`sub`, `iss`, `aud`, `iat`, `exp`, `roles`, `email`) in `extra_claims` are ignored.

## Error responses

Follows RFC 6749 OAuth 2.0 error format:

```json
{
  "error": "invalid_grant",
  "error_description": "invalid username or password"
}
```

## License

Apache-2.0
