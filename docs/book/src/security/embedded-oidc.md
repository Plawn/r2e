# Embedded OAuth/JWT Issuer

`r2e-oidc` provides a local OAuth-style JWT issuer embedded directly in your application. It issues RS256 access tokens without requiring an external identity provider (Keycloak, Auth0, etc.). Ideal for development, prototyping, and monolithic applications.

It includes a local browser login and Authorization Code flow with mandatory
PKCE S256 for registered public clients. It is still not a federated OpenID
Provider: it does not issue ID tokens, implement upstream SSO, or dynamically
register clients. Use an external provider for those cases.

## Installation

Enable the `oidc` feature:

```toml
r2e = { version = "0.3", features = ["security", "oidc"] }
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
    .enable_password_grant_for_development()
    .with_user_store(users);

AppBuilder::new()
    .plugin(oidc)                              // provides Arc<JwtClaimsValidator>
    .build_state().await
    .register_controller::<UserController>()
    .serve("0.0.0.0:3000").await.unwrap();
```

That's it. `AuthenticatedUser` works immediately — no need to manually configure a `JwtClaimsValidator`.

## How it works

`OidcServer` is a `Plugin`. During installation it:

1. **Generates an RSA-2048 key pair** for signing tokens
2. **Creates a `JwtClaimsValidator`** with the public key and injects it into the bean graph
3. **Registers authorization, token, metadata, JWKS, and userinfo endpoints** via a deferred action (after state construction)

Issued tokens are validated locally — no network requests, no JWKS cache.

## Hot-reload support (`OidcRuntime`)

By default, `OidcServer` regenerates RSA keys and rebuilds internal state on each call to `install()`. With hot-reload (`r2e dev`), `main()` is re-executed on each code patch, which invalidates all previously issued tokens and loses in-memory data (user store, client registry).

`OidcServer::build()` separates the expensive construction (once) from route registration (on each patch). It returns an `OidcRuntime` — a `Clone`-able handle that preserves RSA keys, the user store, and the client registry across hot-reload cycles.

```rust
use r2e::prelude::*;
use r2e::r2e_oidc::{OidcServer, InMemoryUserStore, OidcUser};

// App::setup() — called once, before the hot-reload loop
let users = InMemoryUserStore::new()
    .add_user("alice", "password123", OidcUser {
        sub: "user-1".into(),
        roles: vec!["admin".into()],
        ..Default::default()
    });

let oidc = OidcServer::new()
    .enable_password_grant_for_development()
    .with_user_store(users)
    .build(); // returns OidcRuntime

// App::build(b, env) — called on each hot-patch
AppBuilder::new()
    .plugin(oidc.clone()) // reuses the same keys and state
    .build_state().await
    .register_controller::<UserController>()
    .serve("0.0.0.0:3000").await.unwrap();
```

Using `OidcServer` directly as a plugin (without `.build()`) works. Persist a signing key with `.with_signing_key_pem(...)`, or build one `OidcRuntime` in setup, if tokens must survive reloads/restarts.

`oidc.claims_validator()` exposes the same local validator for another
in-process transport. Passing it to an MCP token validator with MCP discovery
disabled keeps the whole development OAuth flow in one process, without a
self-JWKS request or a Docker identity provider.

## Exposed endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` / `POST` | `/oauth/authorize` | Local login + one-time authorization code for registered public clients |
| `POST` | `/oauth/token` | Token issuance (`authorization_code` + PKCE, `client_credentials`; optional development `password`) |
| `GET` | `/.well-known/openid-configuration` | Local issuer metadata |
| `GET` | `/.well-known/jwks.json` | Public key in JWKS format |
| `GET` / `POST` | `/userinfo` | User information (requires a user Bearer token with `openid` scope) |

### Obtaining a token (development password grant)

The password grant is disabled by default. Enable it explicitly only for local fixtures:

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

### Authorization Code + PKCE

Register a public client with an exact redirect allowlist. Plain PKCE and
wildcard redirects are intentionally unsupported:

```rust
use r2e::r2e_oidc::ClientRegistry;

let clients = ClientRegistry::new()
    .add_public_client("mcp-client", ["http://127.0.0.1:49152/callback"])
    .with_scopes(["openid", "profile", "mcp:read"]);

let oidc = OidcServer::new()
    .audience("http://localhost:3000/mcp")
    .with_client_registry(clients)
    .with_user_store(users);
```

Discovery then advertises `/oauth/authorize`, the `authorization_code` grant,
token endpoint auth method `none`, and `code_challenge_methods_supported:
["S256"]`. The browser endpoint authenticates against the configured
`UserStore`; issued codes expire after 300 seconds by default, are bound to
the client, redirect URI, resource and challenge, and are consumed on the
first redemption attempt. Override the lifetime with
`.authorization_code_ttl(seconds)`.

Plain HTTP redirect URIs are accepted only on the loopback interface (RFC 8252
§7.3) — `localhost`, `127.0.0.1` and `[::1]` all qualify; anything else must
use `https`.

#### Authorization errors

Once `client_id` is registered **and** `redirect_uri` matches the allowlist
exactly, protocol errors are reported by redirecting back to the client
(RFC 6749 §4.1.2.1): `303` to
`redirect_uri?error=...&error_description=...&state=...`, echoing the `state`
the client sent.

| Condition | `error` |
|---|---|
| `response_type` other than `code` | `unsupported_response_type` |
| missing `code_challenge`, or `code_challenge_method` != `S256` | `invalid_request` |
| `resource` that is not the configured audience | `invalid_target` |
| a scope outside the client allowlist | `invalid_scope` |
| wrong username/password on the login POST | `access_denied` |

When the client or the redirect URI cannot be validated, the request is answered
with a `400` JSON error instead — never a redirect, so an unregistered
`redirect_uri` can never be used as an open redirect. The token endpoint always
answers with JSON. Every authorize response carrying a code or an error is sent
with `Cache-Control: no-store` and `Pragma: no-cache` (RFC 6749 §5.1).

#### Login CSRF protection

The login page embeds a one-time `csrf_token` hidden field (random, held
server-side, 10-minute TTL). `POST /oauth/authorize` requires it and consumes it
*before* verifying credentials, so a cross-site POST never reaches the password
check. A missing, forged, expired or replayed token is rejected with
`400 invalid_request`; reloading the sign-in page issues a fresh one.

### Scopes

Every client carries a **scope allowlist**, and a freshly registered client
starts with an empty one (fail closed) — it can then only be granted an empty
scope. Declare what a client may ask for with `with_scopes`:

```rust
let clients = ClientRegistry::new()
    .add_public_client("mcp-client", ["http://127.0.0.1:49152/callback"])
    .with_scopes(["openid", "mcp:read"])   // applies to `mcp-client`
    .add_client("worker", "worker-secret")
    .with_scopes(["jobs:run"]);            // applies to `worker`
```

`with_scopes` targets the **most recently registered** client and replaces any
previous allowlist; `try_with_scopes` returns an error instead of panicking
(invalid scope token, or no client registered yet).

- A requested scope outside the allowlist is rejected with `invalid_scope`
  (RFC 6749 §5.2) — on `/oauth/authorize`, `client_credentials` and the
  development password grant alike.
- A request that omits `scope` is granted the whole applicable allowlist.
- `scopes_supported` in the discovery document is the union of every registered
  allowlist (plus the password-grant list when that grant is enabled).

The password grant is not client-authenticated, so it cannot borrow a client's
allowlist. It is bounded by a server-level list instead, defaulting to
`openid profile email roles`:

```rust
let oidc = OidcServer::new()
    .enable_password_grant_for_development()
    .password_grant_scopes(["openid", "profile"])
    .with_user_store(users);
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
    .authorization_code_ttl(300)            // one-time code lifetime
    .base_path("/auth")                     // endpoint prefix (default: "")
    .with_signing_key_pem(private_key_pem)   // persist keys across process restarts
    .max_credential_verifications(16)        // bound concurrent Argon2 work
    .with_user_store(users);
```

With `base_path("/auth")`, the endpoints become:

- `POST /auth/oauth/token`
- `GET|POST /auth/oauth/authorize`
- `GET /auth/.well-known/openid-configuration`
- `GET /auth/.well-known/jwks.json`
- `GET /auth/userinfo`

The canonical JWT issuer also includes the base path. For example, issuer `https://myapp.example.com` plus `base_path("/auth")` yields `iss = "https://myapp.example.com/auth"`.

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

`extra_claims` are merged into the JWT. Reserved claims (`sub`, `iss`, `aud`, `iat`, `exp`, `nbf`, `jti`, `roles`, `email`, `scope`, `token_use`, `principal_type`, `client_id`) are ignored to avoid conflicts.

### Custom user store

Implement the `UserStore` trait to use your own backend (SQLx, Redis, LDAP, etc.):

```rust
use r2e::r2e_oidc::{OidcUser, StoreResult, UserStore, UserStoreError};

struct SqlxUserStore {
    pool: sqlx::SqlitePool,
}

impl UserStore for SqlxUserStore {
    async fn find_by_username(&self, username: &str) -> StoreResult<Option<OidcUser>> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT sub, email, roles FROM users WHERE username = ?"
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| UserStoreError::new(e.to_string()))?;

        Ok(row.map(|row| OidcUser {
            sub: row.sub,
            email: Some(row.email),
            roles: serde_json::from_str(&row.roles).unwrap_or_default(),
            ..Default::default()
        }))
    }

    async fn verify_password(&self, username: &str, password: &str) -> StoreResult<bool> {
        let hash = sqlx::query_scalar::<_, String>(
            "SELECT password_hash FROM users WHERE username = ?"
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| UserStoreError::new(e.to_string()))?;

        Ok(match hash {
            Some(h) => verify_argon2(&h, password),
            None => false,
        })
    }

    async fn find_by_sub(&self, sub: &str) -> StoreResult<Option<OidcUser>> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT sub, email, roles FROM users WHERE sub = ?"
        )
        .bind(sub)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| UserStoreError::new(e.to_string()))?;

        Ok(row.map(|row| OidcUser {
            sub: row.sub,
            email: Some(row.email),
            roles: serde_json::from_str(&row.roles).unwrap_or_default(),
            ..Default::default()
        }))
    }

    async fn authenticate(&self, username: &str, password: &str) -> StoreResult<Option<OidcUser>> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT sub, email, roles, password_hash FROM users WHERE username = ?"
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| UserStoreError::new(e.to_string()))?;

        let Some(row) = row else {
            return Ok(None);
        };

        if !verify_argon2(&row.password_hash, password) {
            return Ok(None);
        }

        Ok(Some(OidcUser {
            sub: row.sub,
            email: Some(row.email),
            roles: serde_json::from_str(&row.roles).unwrap_or_default(),
            ..Default::default()
        }))
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
    .with_scopes(["orders:read", "orders:write"])
    .add_client("batch-worker", "worker-secret")
    .with_scopes(["jobs:run"]);

let oidc = OidcServer::new()
    .with_user_store(users)
    .with_client_registry(clients);
```

Client secrets are also hashed with Argon2. A client without `with_scopes`
receives no scope at all, and `scope=orders:read` is only honored for a client
that declares it — anything else is `invalid_scope`.

### Obtaining a client token

```bash
curl -X POST http://localhost:3000/oauth/token \
  -u "my-service:service-secret-key" \
  -d "grant_type=client_credentials"
```

`client_secret_post` is still accepted for compatibility:

```bash
curl -X POST http://localhost:3000/oauth/token \
  -d "grant_type=client_credentials" \
  -d "client_id=my-service" \
  -d "client_secret=service-secret-key"
```

The issued token has `sub = "client:<client_id>"`, `principal_type = "client"`, and is rejected by `/userinfo`.

## JWT claims

Issued tokens contain the following claims:

| Claim | Source | Description |
|-------|--------|-------------|
| `sub` | `OidcUser.sub` / `client:<client_id>` | Unique subject identifier |
| `iss` | Canonical issuer | Token issuer |
| `aud` | Configuration | Target audience |
| `iat` | Automatic | Issued-at timestamp |
| `exp` | Configuration | Expiration timestamp |
| `scope` | Request/default | Granted scopes |
| `token_use` | Automatic | `access` |
| `principal_type` | Automatic | `user` or `client` |
| `client_id` | Client registry | Present on machine tokens |
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
| `invalid_token` | 401 | Missing or invalid token (userinfo) |
| `insufficient_scope` | 403 | Valid token without required scope |
| `server_error` | 500 | Internal error |

## Full example

```rust
use r2e::prelude::*;
use r2e::r2e_oidc::{OidcServer, InMemoryUserStore, OidcUser, ClientRegistry};
use std::collections::HashMap;
use serde_json::json;

#[controller(path = "/api")]
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

#[r2e::main]
async fn main() {
    let users = InMemoryUserStore::new()
        .add_user("alice", "pass", OidcUser {
            sub: "u1".into(),
            email: Some("alice@example.com".into()),
            roles: vec!["admin".into()],
            ..Default::default()
        });

    let clients = ClientRegistry::new()
        .add_client("worker", "worker-secret")
        .with_scopes(["jobs:run"]);

    let oidc = OidcServer::new()
        .issuer("http://localhost:3000")
        .enable_password_grant_for_development()
        .with_user_store(users)
        .with_client_registry(clients);

    AppBuilder::new()
        .plugin(oidc)
        .register::<UserService>()
        .plugin(Health)
        .plugin(Tracing)
        .build_state().await
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

let oidc = OidcServer::new()
    .enable_password_grant_for_development()
    .with_user_store(users);

let app = TestApp::from_builder(
    AppBuilder::new()
        .plugin(oidc)
        .build_state().await
        .register_controller::<MyController>(),
);

// 1. Obtain a token
let token_resp = app.post("/oauth/token")
    .form(&[
        ("grant_type", "password"),
        ("username", "test-user"),
        ("password", "test-pass"),
    ])
    .send()
    .await;
token_resp.assert_ok();
let token: serde_json::Value = token_resp.json();
let access_token = token["access_token"].as_str().unwrap();

// 2. Use the token
app.get("/api/me")
    .bearer(access_token)
    .send()
    .await
    .assert_ok();
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
