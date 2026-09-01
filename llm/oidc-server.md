---
topic: oidc-server
features: oidc
tokens: ~1300
requires: security
---

## OIDC Server (Embedded)

### TL;DR

- Enable feature `oidc` and install the `OidcServer::new()` plugin — it
  auto-provides `Arc<JwtClaimsValidator>`, so `AuthenticatedUser` needs no
  manual wiring (see llm/security.md).
- It serves `POST /oauth/token`, `GET|POST /oauth/authorize`,
  `/.well-known/openid-configuration`, `/.well-known/jwks.json`, `GET /userinfo`.
- Declare users with `InMemoryUserStore::new().add_user(...)` +
  `.with_user_store(users)`; declare clients with `ClientRegistry`
  (`add_public_client` for PKCE, `add_client` for a secret) +
  `.with_client_registry(clients)`.
- Scopes are allowlisted **per client and start empty (fail closed)**: without
  `with_scopes` a client can only receive an empty scope, and an out-of-list
  scope is a 400 `invalid_scope`.
- `with_scopes` applies to the **most recently registered** client and replaces
  its previous list (`try_with_scopes` returns a `Result` instead of panicking).
- The development password grant (`enable_password_grant_for_development()`)
  uses the server-level `password_grant_scopes`, never a client's allowlist.
- Send the RFC 8707 `resource` parameter to mint a token whose `aud` is that
  resource (e.g. an MCP resource-server token); a relative URI or one with a
  fragment is a 400 `invalid_target`.
- Public-client redirect URIs are **exact match**, and plain `http` is accepted
  only for loopback (`localhost`, `127.0.0.1`, `[::1]`).
- Once `client_id` and `redirect_uri` validate, `/oauth/authorize` reports
  protocol errors by **redirecting** (303 with `error=…`); when they do not, it
  stays a 400 JSON error (no open redirect).
- `POST /oauth/authorize` consumes the login page's one-time `csrf_token`
  (10-min TTL) before checking credentials — missing, forged or replayed is a
  400 `invalid_request`.

Requires feature: `oidc`. Issues JWTs without an external IdP; auto-provides
`Arc<JwtClaimsValidator>` so `AuthenticatedUser` works with no manual setup.

```rust
use r2e::r2e_oidc::{OidcServer, InMemoryUserStore, OidcUser};

# fn __doc(b: AppBuilder) -> impl Sized {
let users = InMemoryUserStore::new()
    .add_user("alice", "password123", OidcUser {
        sub: "user-1".into(),
        email: Some("alice@example.com".into()),
        roles: vec!["admin".into()],
        ..Default::default()
    });

b.plugin(OidcServer::new()
    .issuer("http://localhost:3000")
    .audience("my-app")
    .token_ttl(3600)
    .with_user_store(users))
# }
```

Endpoints: `POST /oauth/token`, `GET|POST /oauth/authorize`,
`/.well-known/openid-configuration`, `/.well-known/jwks.json`, `GET /userinfo`.

`POST /oauth/token` honors `scope` (space-separated, normalized into the
token's `scope` claim) and the RFC 8707 `resource` indicator: when present it
becomes the token's `aud` (instead of the configured audience) — handy for
minting MCP resource-server tokens (`resource=http://localhost:3000/mcp`). An
invalid `resource` (relative URI, or one carrying a fragment) is a 400
`invalid_target`.

**Scopes are allowlisted per client, and every client starts empty (fail
closed)** — a client with no `with_scopes` can only receive an empty scope.
`with_scopes` applies to the **most recently registered** client and replaces
any previous list (`try_with_scopes` returns `Result` instead of panicking):

```rust
use r2e::r2e_oidc::ClientRegistry;

# fn __doc(b: AppBuilder, users: r2e::r2e_oidc::InMemoryUserStore) -> impl Sized {
let clients = ClientRegistry::new()
    .add_public_client("mcp-client", ["http://127.0.0.1:49152/callback"])
    .with_scopes(["openid", "mcp:read"])   // -> mcp-client
    .add_client("worker", "worker-secret")
    .with_scopes(["jobs:run"]);            // -> worker

b.plugin(OidcServer::new()
    .audience("http://localhost:3000/mcp")
    .enable_password_grant_for_development()
    .password_grant_scopes(["openid", "profile"])  // default: openid profile email roles
    .with_user_store(users)
    .with_client_registry(clients))
# }
```

A requested scope outside the allowlist is a 400 `invalid_scope` (RFC 6749
§5.2) on `authorization_code`, `client_credentials` and the development
`password` grant. Omitting `scope` grants the whole applicable allowlist. The
password grant is not client-authenticated, so it uses the server-level
`password_grant_scopes` list, never a client's. Discovery `scopes_supported` is
the union of every allowlist.

Public clients: redirect URIs are exact-match only; plain `http` is accepted
only for loopback (`localhost`, `127.0.0.1`, `[::1]`). Once `client_id` is
registered **and** `redirect_uri` matches exactly, `/oauth/authorize` reports
protocol errors by redirecting (RFC 6749 §4.1.2.1) —
`303 redirect_uri?error=...&error_description=...&state=...` with
`unsupported_response_type` (bad `response_type`), `invalid_request` (missing
`code_challenge` / non-`S256` method), `invalid_target` (wrong `resource`),
`invalid_scope`, or `access_denied` (bad credentials on the login POST). When
the client or redirect URI cannot be validated, it stays a 400 JSON error (no
open redirect). Code and error redirects carry `Cache-Control: no-store`.

The login page embeds a one-time `csrf_token` hidden field (10-min TTL) that
`POST /oauth/authorize` requires and consumes before checking credentials;
missing/forged/replayed = 400 `invalid_request`.
