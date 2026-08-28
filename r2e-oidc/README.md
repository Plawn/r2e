# r2e-oidc

Embedded OAuth/JWT issuer plugin for R2E — issue local RS256 access tokens without an external identity provider.

## Overview

Provides a local OAuth-style token issuer that runs inside your application. It generates or loads RSA-2048 keys, exposes token/JWKS/userinfo metadata endpoints, and automatically provides `Arc<JwtClaimsValidator>` to the bean graph so `AuthenticatedUser` works out-of-the-box.

Ideal for development, testing, prototyping, and monolithic applications that don't need an external IdP.

The embedded login supports public clients through Authorization Code +
mandatory PKCE S256. This is still a focused access-token issuer, not a full
federated OpenID Provider: there are no ID tokens, upstream SSO, or dynamic
client registration.

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
    .enable_password_grant_for_development()
    .with_user_store(users);

AppBuilder::new()
    .plugin(oidc)
    .build_state().await
    .register_controller::<UserController>()
    .serve("0.0.0.0:3000").await.unwrap();
```

No manual `JwtClaimsValidator` setup required — the plugin provides it automatically.

## Hot-reload support (`OidcRuntime`)

By default, `OidcServer::install()` generates RSA keys and builds internal state on every call. With hot-reload (`r2e dev`), `main()` is re-executed on each code patch, which would regenerate keys and invalidate all previously issued tokens.

`OidcServer::build()` separates the expensive one-time setup from route registration. It returns an `OidcRuntime` — a `Clone`-able handle that preserves RSA keys, user store, and client registry across hot-reload cycles.

```rust
use r2e::prelude::*;
use r2e::r2e_oidc::{OidcServer, InMemoryUserStore, OidcUser};

// setup() — called once, before hot-reload loop
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

// main(env) — called on each hot-patch
AppBuilder::new()
    .plugin(oidc.clone()) // reuses same keys and state
    .build_state().await
    .register_controller::<UserController>()
    .serve("0.0.0.0:3000").await.unwrap();
```

Using `OidcServer` directly as a plugin (without `.build()`) still works. Persist a signing key with `.with_signing_key_pem(...)`, or build one `OidcRuntime` in setup, if tokens must survive reloads/restarts.

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` / `POST` | `/oauth/authorize` | Local login and Authorization Code + PKCE |
| `POST` | `/oauth/token` | Token issuance (`authorization_code`, `client_credentials`; optional development `password`) |
| `GET` | `/.well-known/openid-configuration` | Local issuer metadata |
| `GET` | `/.well-known/jwks.json` | Public key in JWKS format |
| `GET` / `POST` | `/userinfo` | User info (requires a user Bearer token with `openid` scope) |

## Configuration

```rust
OidcServer::new()
    .issuer("https://myapp.example.com")   // JWT `iss` claim (default: "http://localhost:3000")
    .audience("my-app")                     // JWT `aud` claim (default: "r2e-app")
    .token_ttl(7200)                        // Token TTL in seconds (default: 3600)
    .authorization_code_ttl(300)            // One-time code TTL (default: 300)
    .base_path("/auth")                     // Endpoint prefix (default: "")
    .with_signing_key_pem(private_key_pem)   // Persist keys across process restarts
    .max_credential_verifications(16)        // Bound concurrent Argon2 work
    .with_user_store(users)
```

With `base_path("/auth")`, endpoints become `/auth/oauth/token`, `/auth/.well-known/openid-configuration`, etc., and the canonical JWT issuer becomes `https://myapp.example.com/auth`.

The resource owner password grant is disabled by default. Enable it explicitly only for local fixtures:

```rust
let oidc = OidcServer::new()
    .enable_password_grant_for_development()
    .with_user_store(users);
```

## Authorization Code + PKCE

```rust
use r2e::r2e_oidc::ClientRegistry;

let clients = ClientRegistry::new()
    .add_public_client("mcp-client", ["http://127.0.0.1:49152/callback"])
    .with_scopes(["openid", "profile", "mcp:read"]);

let oidc = OidcServer::new()
    .audience("http://localhost:3000/mcp")
    .with_user_store(users)
    .with_client_registry(clients);
```

Redirect URIs are exact-match only; non-loopback HTTP redirects are rejected
(`localhost`, `127.0.0.1` and `[::1]` are all recognized as loopback).
Discovery advertises only PKCE `S256`. Authorization codes expire after five
minutes by default, are bound to client/redirect/resource/challenge, and are
removed on the first redemption attempt.

### Authorization errors

Once `client_id` is registered **and** `redirect_uri` matches the allowlist
exactly, `/oauth/authorize` reports protocol errors by redirecting back to the
client (RFC 6749 §4.1.2.1) — `303` to
`redirect_uri?error=...&error_description=...&state=...`:

| Condition | `error` |
|---|---|
| `response_type` other than `code` | `unsupported_response_type` |
| missing `code_challenge`, or `code_challenge_method` != `S256` | `invalid_request` |
| `resource` that is not the configured audience | `invalid_target` |
| a scope outside the client allowlist | `invalid_scope` |
| wrong username/password on the login POST | `access_denied` |

When the client or the redirect URI cannot be validated, the request is
answered with a `400` JSON error instead — never a redirect (no open redirect).
All authorize responses that carry a code or an error are sent with
`Cache-Control: no-store` and `Pragma: no-cache`.

### Login CSRF protection

The login page embeds a one-time `csrf_token` hidden field (random, server-side,
10-minute TTL). `POST /oauth/authorize` requires it and consumes it before any
credential check; a missing, forged, expired or replayed token is rejected with
`400 invalid_request`. Reload the sign-in page to get a fresh one.

## Scopes

Every client carries a **scope allowlist** and starts empty (fail closed): a
client with no `.with_scopes(...)` can only receive an empty scope.

```rust
let clients = ClientRegistry::new()
    .add_public_client("mcp-client", ["http://127.0.0.1:49152/callback"])
    .with_scopes(["openid", "mcp:read"])   // applies to `mcp-client`
    .add_client("worker", "worker-secret")
    .with_scopes(["jobs:run"]);            // applies to `worker`
```

`with_scopes` applies to the **most recently registered** client and replaces
any previous allowlist (`try_with_scopes` returns an error instead of
panicking). A requested scope outside the allowlist is rejected with
`invalid_scope` (RFC 6749 §5.2) on `/oauth/authorize`, `client_credentials` and
the development password grant. A request that omits `scope` is granted the
whole applicable allowlist. `scopes_supported` in the discovery document is the
union of every registered allowlist.

The password grant is not client-authenticated, so it cannot borrow a client's
allowlist; it is bounded by a server-level list instead (default
`openid profile email roles`):

```rust
OidcServer::new()
    .enable_password_grant_for_development()
    .password_grant_scopes(["openid", "profile"])
    .with_user_store(users);
```

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
use r2e::r2e_oidc::{OidcUser, StoreResult, UserStore};

struct SqlxUserStore { pool: SqlitePool }

impl UserStore for SqlxUserStore {
    async fn find_by_username(&self, username: &str) -> StoreResult<Option<OidcUser>> { /* ... */ }
    async fn verify_password(&self, username: &str, password: &str) -> StoreResult<bool> { /* ... */ }
    async fn find_by_sub(&self, sub: &str) -> StoreResult<Option<OidcUser>> { /* ... */ }
    async fn authenticate(&self, username: &str, password: &str) -> StoreResult<Option<OidcUser>> {
        /* verify and return the user in one backend operation */
    }
}
```

## Client credentials grant

For service-to-service communication, register OAuth clients:

```rust
use r2e::r2e_oidc::ClientRegistry;

let clients = ClientRegistry::new()
    .add_client("my-service", "service-secret")
    .with_scopes(["jobs:read", "jobs:run"]);

let oidc = OidcServer::new()
    .with_user_store(users)
    .with_client_registry(clients);
```

```bash
curl -X POST http://localhost:3000/oauth/token \
  -u "my-service:service-secret" \
  -d "grant_type=client_credentials"
```

`client_secret_post` is still accepted for compatibility:

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
| `sub` | `OidcUser.sub` or `client:<client_id>` |
| `iss` | Canonical issuer |
| `aud` | Configuration |
| `iat` / `exp` | Automatic |
| `token_use` | `access` |
| `principal_type` | `user` or `client` |
| `client_id` | Client identifier for machine tokens |
| `scope` | Granted scopes |
| `roles` | `OidcUser.roles` |
| `email` | `OidcUser.email` (if set) |
| *custom* | `OidcUser.extra_claims` |

Reserved claims (`sub`, `iss`, `aud`, `iat`, `exp`, `nbf`, `jti`, `roles`, `email`, `scope`, `token_use`, `principal_type`, `client_id`) in `extra_claims` are ignored.

## Error responses

Follows RFC 6749 OAuth 2.0 error format:

```json
{
  "error": "invalid_grant",
  "error_description": "invalid username or password"
}
```

`/oauth/token` always answers with JSON. `/oauth/authorize` answers with a
redirect once the client and redirect URI are validated — see
[Authorization errors](#authorization-errors).

## License

Apache-2.0
