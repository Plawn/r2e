# Step 4 — r2e-security: JWT and Identity

## Goal

Implement the security module: JWT validation, JWKS cache, and the `AuthenticatedUser` extractor compatible with Axum (`FromRequestParts`).

## Files to Create

```
r2e-security/src/
  lib.rs              # Re-exports
  identity.rs         # AuthenticatedUser struct
  jwt.rs              # JWT validation, claims decoding
  jwks.rs             # JWKS cache (OIDC public keys)
  extractor.rs        # impl FromRequestParts for AuthenticatedUser
  config.rs           # Security configuration (issuer, audience, JWKS URL)
```

## 1. Configuration (`config.rs`)

```rust
#[derive(Clone, Debug)]
pub struct SecurityConfig {
    /// JWKS endpoint URL (e.g., https://auth.example.com/.well-known/jwks.json)
    pub jwks_url: String,

    /// Expected issuer in the "iss" claim
    pub issuer: String,

    /// Expected audience in the "aud" claim
    pub audience: String,

    /// JWKS cache duration in seconds (default: 3600)
    pub jwks_cache_ttl_secs: u64,
}
```

The `SecurityConfig` will be stored in the `AppState` to be accessible to extractors.

## 2. AuthenticatedUser (`identity.rs`)

```rust
#[derive(Clone, Debug, serde::Serialize)]
pub struct AuthenticatedUser {
    /// Subject ("sub" claim)
    pub sub: String,

    /// Email ("email" claim, optional)
    pub email: Option<String>,

    /// Roles extracted from claims
    pub roles: Vec<String>,

    /// Raw claims for advanced access
    pub claims: serde_json::Value,
}
```

### Role Extraction

Roles can come from different locations depending on the OIDC provider:

- Keycloak: `realm_access.roles` or `resource_access.<client>.roles`
- Auth0: custom claim `https://example.com/roles`
- Generic: `roles` claim

Provide a configurable `RoleExtractor` trait with a default implementation that looks in `roles`, `realm_access.roles`.

## 3. JWKS Cache (`jwks.rs`)

### Responsibilities

1. Download public keys from the JWKS endpoint
2. Index by `kid` (Key ID)
3. Cache keys with a configurable TTL
4. Refresh in the background when the TTL expires

### Implementation

```rust
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use jsonwebtoken::DecodingKey;

pub struct JwksCache {
    keys: Arc<RwLock<HashMap<String, DecodingKey>>>,
    config: SecurityConfig,
    client: reqwest::Client,
}

impl JwksCache {
    pub async fn new(config: SecurityConfig) -> Result<Self, SecurityError> { ... }

    /// Retrieves the decoding key for a given kid.
    /// Refreshes the cache if the kid is unknown.
    pub async fn get_key(&self, kid: &str) -> Result<DecodingKey, SecurityError> { ... }

    /// Forces a cache refresh.
    async fn refresh(&self) -> Result<(), SecurityError> { ... }
}
```

### Refresh Strategy

1. If `kid` is found in cache → return directly
2. If `kid` is unknown → refresh the cache then search again
3. If still unknown after refresh → error `UnknownKeyId`

## 4. JWT Validation (`jwt.rs`)

```rust
pub struct JwtValidator {
    jwks: Arc<JwksCache>,
    config: SecurityConfig,
}

impl JwtValidator {
    /// Validates a JWT token and returns an AuthenticatedUser
    pub async fn validate(&self, token: &str) -> Result<AuthenticatedUser, SecurityError> {
        // 1. Decode the header to extract the kid
        // 2. Retrieve the key from the JWKS cache
        // 3. Validate the signature
        // 4. Validate claims (iss, aud, exp, nbf)
        // 5. Map claims to AuthenticatedUser
        ...
    }
}
```

### Validated Claims

| Claim | Validation |
|-------|-----------|
| `iss` | Must match `config.issuer` |
| `aud` | Must contain `config.audience` |
| `exp` | Must be in the future |
| `nbf` | Must be in the past (if present) |

## 5. Axum Extractor (`extractor.rs`)

```rust
// Note: these types are re-exported via r2e::prelude::*
use r2e_core::http::extract::{FromRequestParts, FromRef};
use r2e_core::http::header::Parts;

impl<S> FromRequestParts<S> for AuthenticatedUser
where
    S: Send + Sync,
    // The state must provide a JwtValidator
    JwtValidator: FromRef<S>,
{
    type Rejection = HttpError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        // 1. Extract the Authorization header
        // 2. Verify the "Bearer" scheme
        // 3. Extract the token
        // 4. Validate via JwtValidator
        // 5. Return AuthenticatedUser or 401
        ...
    }
}
```

### Token Extraction

```
Authorization: Bearer eyJhbGciOiJSUzI1NiIs...
                      ^^^^^^^^^^^^^^^^^^^^^^^^^
                      token extracted here
```

### Possible Errors

| Case | HTTP Code | Message |
|------|-----------|---------|
| Missing header | 401 | Missing Authorization header |
| Scheme != Bearer | 401 | Invalid authorization scheme |
| Invalid token | 401 | Invalid token |
| Expired token | 401 | Token expired |
| Unknown kid | 401 | Unknown signing key |
| Invalid issuer/audience | 401 | Token validation failed |

## 6. Security Errors

```rust
pub enum SecurityError {
    MissingAuthHeader,
    InvalidAuthScheme,
    InvalidToken(String),
    TokenExpired,
    UnknownKeyId(String),
    JwksFetchError(String),
    ValidationFailed(String),
}
```

Implement `IntoResponse` for `SecurityError` → all mapped to 401.

## 7. Integration with AppState

For the extractor to work, the `JwtValidator` must be accessible from the Axum state. Two approaches:

**Approach A** — `FromRef`: the user implements `FromRef<AppState<T>>` for `JwtValidator`

**Approach B (recommended)** — Axum extension: store the `JwtValidator` in the `Router` extensions via a Tower layer.

## Validation Criteria

Unit test with a locally signed JWT (RSA key generated in test):

```rust
#[tokio::test]
async fn test_jwt_validation() {
    let (encoding_key, decoding_key) = generate_test_rsa_keys();
    let token = create_test_jwt(&encoding_key, "user123", "test@example.com");
    let validator = JwtValidator::new_with_static_key(decoding_key, config);
    let user = validator.validate(&token).await.unwrap();
    assert_eq!(user.sub, "user123");
}
```

## Dependencies Between Steps

- Requires: step 0, step 1 (HttpError, AppState)
- Blocks: step 5 (example-app, for full integration)
- Can be done in parallel with steps 2 and 3
