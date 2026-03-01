# Embedded OIDC Server

`r2e-oidc` provides an OIDC server embedded directly in your application. It issues JWT tokens without requiring an external identity provider (Keycloak, Auth0, etc.). Ideal for development, prototyping, and monolithic applications.

## Installation

Enable the `oidc` feature:

```toml
r2e = { version = "0.1", features = ["security", "oidc"] }
```

## Quick start

```rust
use r2e::prelude::*;
use r2e::r2e_oidc::{OidcServer, InMemoryUserStore, OidcUser};

let users = InMemoryUserStore::new()
    .add_user("alice", "password123", OidcUser {
        sub: "user-1".into(),
        email: Some("alice@example.com".into()),
        roles: vec!["admin".into()],
        ..Default::default()
    })
    .add_user("bob", "secret456", OidcUser {
        sub: "user-2".into(),
        email: Some("bob@example.com".into()),
        roles: vec!["user".into()],
        ..Default::default()
    });

let oidc = OidcServer::new()
    .with_user_store(users);

AppBuilder::new()
    .plugin(oidc)                              // pre-state: provides Arc<JwtClaimsValidator>
    .build_state::<Services, _, _>().await
    .register_controller::<UserController>()
    .serve("0.0.0.0:3000").await.unwrap();
```

That's it. `AuthenticatedUser` works immediately — no need to manually configure a `JwtClaimsValidator`.

## How it works

`OidcServer` is a `PreStatePlugin`. During installation it:

1. **Generates an RSA-2048 key pair** for signing tokens
2. **Creates a `JwtClaimsValidator`** with the public key and injects it into the bean graph
3. **Registers OIDC endpoints** via a deferred action (after state construction)

Issued tokens are validated locally — no network requests, no JWKS cache.

## Hot-reload support (`OidcRuntime`)

By default, `OidcServer` regenerates RSA keys and rebuilds internal state on each call to `install()`. With hot-reload (`r2e dev`), `main()` is re-executed on each code patch, which invalidates all previously issued tokens and loses in-memory data (user store, client registry).

`OidcServer::build()` separates the expensive construction (once) from route registration (on each patch). It returns an `OidcRuntime` — a `Clone`-able handle that preserves RSA keys, the user store, and the client registry across hot-reload cycles.

```rust
use r2e::prelude::*;
use r2e::r2e_oidc::{OidcServer, InMemoryUserStore, OidcUser};

// setup() — called once, before the hot-reload loop
let users = InMemoryUserStore::new()
    .add_user("alice", "password123", OidcUser {
        sub: "user-1".into(),
        roles: vec!["admin".into()],
        ..Default::default()
    });

let oidc = OidcServer::new()
    .with_user_store(users)
    .build(); // returns OidcRuntime

// main(env) — called on each hot-patch
AppBuilder::new()
    .plugin(oidc.clone()) // reuses the same keys and state
    .build_state::<Services, _, _>().await
    .register_controller::<UserController>()
    .serve("0.0.0.0:3000").await.unwrap();
```

**Backward compatibility:** using `OidcServer` directly as a plugin (without `.build()`) works exactly as before. The only difference is that tokens won't survive hot-reload cycles.

## Exposed endpoints

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/oauth/token` | Token issuance (password / client_credentials) |
| `GET` | `/.well-known/openid-configuration` | OpenID Connect discovery document |
| `GET` | `/.well-known/jwks.json` | Public key in JWKS format |
| `GET` | `/userinfo` | User information (requires Bearer token) |

### Obtaining a token (password grant)

```bash
curl -X POST http://localhost:3000/oauth/token \
  -d "grant_type=password" \
  -d "username=alice" \
  -d "password=password123"
```

Response:

```json
{
  "access_token": "eyJhbGciOiJSUzI1NiIs...",
  "token_type": "Bearer",
  "expires_in": 3600
}
```

### Using the token

```bash
curl http://localhost:3000/users/me \
  -H "Authorization: Bearer eyJhbGciOiJSUzI1NiIs..."
```

### Querying userinfo

```bash
curl http://localhost:3000/userinfo \
  -H "Authorization: Bearer eyJhbGciOiJSUzI1NiIs..."
```

Response:

```json
{
  "sub": "user-1",
  "email": "alice@example.com",
  "roles": ["admin"]
}
```

## Configuration

The builder offers several customization options:

```rust
let oidc = OidcServer::new()
    .issuer("https://myapp.example.com")   // `iss` claim (default: "http://localhost:3000")
    .audience("my-app")                     // `aud` claim (default: "r2e-app")
    .token_ttl(7200)                        // lifetime in seconds (default: 3600)
    .base_path("/auth")                     // endpoint prefix (default: "")
    .with_user_store(users);
```

With `base_path("/auth")`, the endpoints become:

- `POST /auth/oauth/token`
- `GET /auth/.well-known/openid-configuration`
- `GET /auth/.well-known/jwks.json`
- `GET /auth/userinfo`

## User store

### InMemoryUserStore

The default in-memory store, suitable for development and testing:

```rust
let users = InMemoryUserStore::new()
    .add_user("alice", "password123", OidcUser {
        sub: "user-1".into(),
        email: Some("alice@example.com".into()),
        roles: vec!["admin".into()],
        extra_claims: HashMap::from([
            ("tenant_id".into(), json!("tenant-42")),
        ]),
    });
```

Passwords are hashed with **Argon2** — plaintext passwords are never stored.

### OidcUser

```rust
pub struct OidcUser {
    pub sub: String,                                    // unique identifier
    pub email: Option<String>,                          // email address
    pub roles: Vec<String>,                             // roles for authorization
    pub extra_claims: HashMap<String, serde_json::Value>, // additional claims
}
```

`extra_claims` are merged into the JWT. Reserved claims (`sub`, `iss`, `aud`, `iat`, `exp`, `roles`, `email`) are ignored to avoid conflicts.

### Custom user store

Implement the `UserStore` trait to use your own backend (SQLx, Redis, LDAP, etc.):

```rust
use r2e::r2e_oidc::{UserStore, OidcUser};

struct SqlxUserStore {
    pool: sqlx::SqlitePool,
}

impl UserStore for SqlxUserStore {
    async fn find_by_username(&self, username: &str) -> Option<OidcUser> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT sub, email, roles FROM users WHERE username = ?"
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await
        .ok()??;

        Some(OidcUser {
            sub: row.sub,
            email: Some(row.email),
            roles: serde_json::from_str(&row.roles).unwrap_or_default(),
            ..Default::default()
        })
    }

    async fn verify_password(&self, username: &str, password: &str) -> bool {
        let hash = sqlx::query_scalar::<_, String>(
            "SELECT password_hash FROM users WHERE username = ?"
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();

        match hash {
            Some(h) => verify_argon2(&h, password),
            None => false,
        }
    }

    async fn find_by_sub(&self, sub: &str) -> Option<OidcUser> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT sub, email, roles FROM users WHERE sub = ?"
        )
        .bind(sub)
        .fetch_optional(&self.pool)
        .await
        .ok()??;

        Some(OidcUser {
            sub: row.sub,
            email: Some(row.email),
            roles: serde_json::from_str(&row.roles).unwrap_or_default(),
            ..Default::default()
        })
    }
}
```

Then use it:

```rust
let store = SqlxUserStore { pool: pool.clone() };
let oidc = OidcServer::new().with_user_store(store);
```

## Client credentials grant

For service-to-service communication, configure a `ClientRegistry`:

```rust
use r2e::r2e_oidc::ClientRegistry;

let clients = ClientRegistry::new()
    .add_client("my-service", "service-secret-key")
    .add_client("batch-worker", "worker-secret");

let oidc = OidcServer::new()
    .with_user_store(users)
    .with_client_registry(clients);
```

Client secrets are also hashed with Argon2.

### Obtaining a client token

```bash
curl -X POST http://localhost:3000/oauth/token \
  -d "grant_type=client_credentials" \
  -d "client_id=my-service" \
  -d "client_secret=service-secret-key"
```

The issued token has the `client_id` as `sub` and an empty `roles` array.

## JWT claims

Issued tokens contain the following claims:

| Claim | Source | Description |
|-------|--------|-------------|
| `sub` | `OidcUser.sub` / `client_id` | Unique subject identifier |
| `iss` | Configuration | Token issuer |
| `aud` | Configuration | Target audience |
| `iat` | Automatic | Issued-at timestamp |
| `exp` | Configuration | Expiration timestamp |
| `roles` | `OidcUser.roles` | User roles |
| `email` | `OidcUser.email` | Email (if set) |
| *custom* | `OidcUser.extra_claims` | Additional claims |

The signing algorithm is **RS256** (RSA + SHA-256).

## Error handling

Error responses follow RFC 6749 (OAuth 2.0):

```json
{
  "error": "invalid_grant",
  "error_description": "invalid username or password"
}
```

| Error code | HTTP | Cause |
|------------|------|-------|
| `invalid_request` | 400 | Missing or invalid parameter |
| `invalid_grant` | 400 | Invalid credentials (password grant) |
| `unsupported_grant_type` | 400 | Unsupported grant type |
| `invalid_client` | 401 | Invalid client credentials |
| `unauthorized` | 401 | Missing or invalid token (userinfo) |
| `server_error` | 500 | Internal error |

## Full example

```rust
use r2e::prelude::*;
use r2e::r2e_oidc::{OidcServer, InMemoryUserStore, OidcUser, ClientRegistry};
use std::collections::HashMap;
use serde_json::json;

#[derive(Clone, BeanState)]
pub struct Services {
    pub claims_validator: Arc<JwtClaimsValidator>,
    pub user_service: UserService,
}

#[derive(Controller)]
#[controller(path = "/api", state = Services)]
pub struct ApiController {
    #[inject] user_service: UserService,
}

#[routes]
impl ApiController {
    #[get("/public")]
    async fn public_data(&self) -> Json<&'static str> {
        Json("accessible to everyone")
    }

    #[get("/me")]
    async fn me(&self, #[inject(identity)] user: AuthenticatedUser) -> Json<AuthenticatedUser> {
        Json(user)
    }

    #[get("/admin")]
    #[roles("admin")]
    async fn admin(&self, #[inject(identity)] user: AuthenticatedUser) -> Json<&'static str> {
        Json("admin data")
    }
}

#[tokio::main]
async fn main() {
    let users = InMemoryUserStore::new()
        .add_user("alice", "pass", OidcUser {
            sub: "u1".into(),
            email: Some("alice@example.com".into()),
            roles: vec!["admin".into()],
            ..Default::default()
        });

    let clients = ClientRegistry::new()
        .add_client("worker", "worker-secret");

    let oidc = OidcServer::new()
        .issuer("http://localhost:3000")
        .with_user_store(users)
        .with_client_registry(clients);

    AppBuilder::new()
        .plugin(oidc)
        .with_bean::<UserService>()
        .build_state::<Services, _, _>().await
        .with(Health)
        .with(Tracing)
        .register_controller::<ApiController>()
        .serve("0.0.0.0:3000").await.unwrap();
}
```

## Testing

`r2e-oidc` integrates naturally with `r2e-test`. Use `OidcServer` in your integration tests:

```rust
use r2e_test::TestApp;
use r2e::r2e_oidc::{OidcServer, InMemoryUserStore, OidcUser};

let users = InMemoryUserStore::new()
    .add_user("test-user", "test-pass", OidcUser {
        sub: "test-1".into(),
        roles: vec!["admin".into()],
        ..Default::default()
    });

let oidc = OidcServer::new().with_user_store(users);

let app = AppBuilder::new()
    .plugin(oidc)
    .build_state::<TestState, _, _>().await
    .register_controller::<MyController>()
    .build();

let client = TestApp::new(app);

// 1. Obtain a token
let token_resp = client.post("/oauth/token")
    .form(&[
        ("grant_type", "password"),
        ("username", "test-user"),
        ("password", "test-pass"),
    ])
    .await;
assert_eq!(token_resp.status(), 200);
let token: serde_json::Value = token_resp.json().await;
let access_token = token["access_token"].as_str().unwrap();

// 2. Use the token
let resp = client.get("/api/me")
    .header("Authorization", format!("Bearer {access_token}"))
    .await;
assert_eq!(resp.status(), 200);
```

> **Tip:** For simple tests that don't need the full OAuth flow, `TestJwt` (see [TestJwt](../testing/test-jwt.md)) remains the fastest way to generate test tokens.

## When to use r2e-oidc vs an external provider

| Scenario | Recommendation |
|----------|---------------|
| Local development | `r2e-oidc` — no external infrastructure needed |
| Integration tests | `r2e-oidc` or `TestJwt` |
| Prototyping / MVP | `r2e-oidc` — simplified deployment |
| Monolithic app without SSO | `r2e-oidc` — built-in user management |
| Production with SSO | External provider (Keycloak, Auth0, etc.) |
| Multi-app / federation | External provider |

Migrating to an external provider is transparent: your controllers use `AuthenticatedUser` in both cases. Only the configuration in `main.rs` changes.
