# Feature 11 — JWT Security / Roles

## TL;DR

Complete JWT auth wired into controllers: token validation, identity extraction, and role-based access control. Provide an `Arc<JwtClaimsValidator>` bean (static HMAC key for tests, JWKS endpoint for prod via `SecurityConfig`), then extract the user with `#[inject(identity)] user: AuthenticatedUser` (struct field or handler param; `Option<_>` for adaptive auth) and gate routes with `#[roles("admin")]`. A struct-level identity authenticates every route fail-closed — opt public routes out explicitly with `#[anonymous]`.


## Goal

Provide complete JWT authentication with token validation, automatic user identity extraction, and role-based access control — all integrated into the controller system via `#[inject(identity)]` and `#[roles]`.

## Key Concepts

### AuthenticatedUser

Struct representing the authenticated user, automatically extracted from the `Authorization: Bearer <token>` header. Extraction goes through R2E's own `FromRequestPartsVia<S, M>` / `OptionalFromRequestPartsVia<S, M>` traits (the marker slot `M` carries the `HasBean` witness that locates the `Arc<JwtClaimsValidator>` bean in the state). Plain axum `FromRequestParts<S>` extractors still work via a blanket `ViaAxum` bridge. Both traits are in the prelude.

### JwtClaimsValidator

Validates JWT tokens — supports static keys (tests, HMAC) and JWKS endpoints (production, RSA/ECDSA). Provided to the application as a bean (`Arc<JwtClaimsValidator>`); every identity extractor resolves it from the bean graph by type.

### SecurityConfig

Validation configuration: JWKS URL, expected issuer, expected audience.

### #[inject(identity)]

`#[controller]` attribute that marks a field as extracted from the request (request scope). The field lives on the generated request façade and is automatically populated by the corresponding `FromRequestPartsVia` extractor. Can also be applied to a handler parameter — `#[inject(identity)] user: AuthenticatedUser` or `Option<AuthenticatedUser>` — so only annotated endpoints require authentication.

### #[roles("...")]

Method attribute that restricts access to users having at least one of the specified roles.

## Usage

### 1. Configuring JwtClaimsValidator

#### Test mode (static HMAC key)

```rust
use r2e::r2e_security::{JwtClaimsValidator, SecurityConfig};
use jsonwebtoken::DecodingKey;

let secret = b"mon-secret-change-en-production";
let config = SecurityConfig::new("unused", "mon-issuer", "mon-audience");
let validator = JwtClaimsValidator::new_with_static_key(
    DecodingKey::from_secret(secret),
    config,
);
```

#### Production mode (JWKS endpoint)

```rust
let config = SecurityConfig::new(
    "https://auth.example.com/.well-known/jwks.json",
    "https://auth.example.com",
    "mon-application",
);
let validator = JwtClaimsValidator::new(config).await?;
```

The `JwksCache` downloads and caches public keys, with automatic refresh.

### 2. Providing the Validator as a Bean

The validator is resolved from the bean graph **by type** — there is no
hand-written state struct and no `FromRef` impl. Provide it once as
`Arc<JwtClaimsValidator>` and every identity extractor (`AuthenticatedUser` and
any custom `FromValidatedJwtClaims`) finds it automatically:

```rust
use std::sync::Arc;

let app = AppBuilder::new()
    .provide(Arc::new(validator))   // Arc<JwtClaimsValidator> — resolved by type
    .register::<UserService>();
```

`AuthenticatedUser` will fail to compile if no `Arc<JwtClaimsValidator>` bean is
present, naming the missing type.

### 3. Using `#[inject(identity)]` in a Controller

```rust
use r2e::prelude::*;
use r2e::r2e_security::AuthenticatedUser;

#[controller(path = "/users")]
pub struct UserController {
    #[inject(identity)]
    user: AuthenticatedUser,
}

#[routes]
impl UserController {
    #[get("/me")]
    async fn me(&self) -> axum::Json<AuthenticatedUser> {
        axum::Json(self.user.clone())
    }
}
```

`AuthenticatedUser` is automatically extracted from each request:
- If the `Authorization` header is missing or the token is invalid -> 401 response
- If the token is valid -> the `user` field is populated

### 4. AuthenticatedUser Structure

```rust
pub struct AuthenticatedUser {
    pub sub: String,              // Subject (ID utilisateur)
    pub email: Option<String>,    // Email (si present dans les claims)
    pub roles: Vec<String>,       // Roles extraits des claims
    pub claims: serde_json::Value, // Claims bruts pour acces avance
}
```

### Role Extraction

Roles are looked up in the following order:
1. `roles` claim (array of strings)
2. `realm_access.roles` claim (Keycloak format)

The `RoleExtractor` trait can be implemented to support other formats.

### Custom Identity Types (`FromValidatedJwtClaims`)

To carry data beyond the raw JWT claims (e.g. a DB-backed profile), implement
`FromValidatedJwtClaims<S>` **generic over the state `S`** — never over a concrete state
struct. Read any beans you need (a pool, a repository, …) from the state via
`state.bean::<T>()` (the `BeanLookup` vocabulary), then call
`impl_claims_identity_extractor!` to generate the `FromRequestPartsVia` glue:

```rust
use r2e::{BeanLookup, Identity};
use r2e::r2e_security::{impl_claims_identity_extractor, AuthenticatedUser, FromValidatedJwtClaims};

impl<S> FromValidatedJwtClaims<S> for DbUser
where
    S: BeanLookup + Send + Sync,
{
    async fn from_jwt_claims(
        claims: serde_json::Value,
        state: &S,
    ) -> Result<Self, r2e::HttpError> {
        let auth = AuthenticatedUser::from_claims(claims);
        let pool = state
            .bean::<sqlx::SqlitePool>()
            .ok_or_else(|| r2e::HttpError::internal("SqlitePool bean not found in state"))?;
        // … fetch profile from `pool` …
        Ok(DbUser { auth, /* profile */ })
    }
}

impl_claims_identity_extractor!(DbUser);
```

For typed application claims, implement `JwtClaimSet` and select the claims
type both on the identity trait and the extractor macro:

```rust
use serde::Deserialize;
use r2e::Identity;
use r2e::r2e_security::{
    impl_claims_identity_extractor, FromValidatedJwtClaims, JwtClaimSet,
};

#[derive(Deserialize)]
struct TenantClaims {
    sub: String,
    tenant_id: String,
}

struct TenantUser {
    sub: String,
    tenant_id: String,
}

impl Identity for TenantUser {
    fn sub(&self) -> &str {
        &self.sub
    }
}

impl JwtClaimSet for TenantClaims {
    fn subject(&self) -> Option<&str> {
        Some(&self.sub)
    }
}

impl<S: Send + Sync> FromValidatedJwtClaims<S, TenantClaims> for TenantUser {
    async fn from_jwt_claims(
        claims: TenantClaims,
        _state: &S,
    ) -> Result<Self, r2e::HttpError> {
        Ok(TenantUser {
            sub: claims.sub,
            tenant_id: claims.tenant_id,
        })
    }
}

impl_claims_identity_extractor!(TenantUser, claims = TenantClaims);
```

This path deserializes the validated payload directly into `TenantClaims` and
does not build an intermediate `serde_json::Value`.

The same `Arc<JwtClaimsValidator>` bean validates the JWT once; the light
(`AuthenticatedUser`) and full (`DbUser`) identities share it.

### 5. Access Control with `#[roles]`

```rust
#[get("/admin/users")]
#[roles("admin")]
async fn admin_list(&self) -> axum::Json<Vec<User>> {
    let users = self.user_service.list().await;
    axum::Json(users)
}
```

If the user does not have the `"admin"` role, the handler returns 403:

```http
HTTP/1.1 403 Forbidden
Content-Type: application/json

{"error": "Insufficient roles"}
```

#### Multiple Roles

```rust
#[roles("admin", "manager")]
```

The user must have **at least one** of the specified roles (OR, not AND).

### 6. Utility Methods

```rust
// Verifier un role
user.has_role("admin");  // → bool

// Verifier si au moins un role est present
user.has_any_role(&["admin", "manager"]);  // → bool
```

## Authentication Flow

```
1. Client envoie: Authorization: Bearer eyJhbGciOiJSUzI1NiIs...
2. Extracteur AuthenticatedUser:
   a. Extraire le header Authorization
   b. Verifier le schema "Bearer"
   c. Extraire le token
   d. Valider via JwtClaimsValidator (signature, exp, iss, aud)
   e. Mapper les claims vers AuthenticatedUser
3. Si valide → handler execute avec self.user peuple
4. Si invalide → 401 retourne, handler jamais execute
```

### Possible Errors

| Case | HTTP Code | Message |
|------|-----------|---------|
| Missing header | 401 | Missing Authorization header |
| Scheme != Bearer | 401 | Invalid authorization scheme |
| Invalid token | 401 | Invalid token |
| Expired token | 401 | Token expired |
| Invalid issuer/audience | 401 | Token validation failed |

## Code Generated by the Macro

For a controller with `#[inject(identity)] user: AuthenticatedUser`, `#[controller]` splits the
struct into an application-scoped **core** (holding only `#[inject]` / `#[config]` fields,
built once at router-build time and shared as an `Arc`) and a per-request **façade**
(`__R2eRequest_UserController`) that holds the request-scoped `user` plus an `Arc` to the
core, with `Deref<Target = Core>`. The core is built by `ContextConstruct::from_context`,
which pulls each `#[inject]` field from the resolved `BeanContext` **by type**. The macros
also generate a `FromRequestParts` extractor `__R2eRequestData_UserController` that produces
**only** the request-scoped values (the identity here); `#[inject]` and `#[config]` live on
the shared core and are not re-resolved per request. Each registered route closure captures
the core `Arc`, extracts the request data, and `bind_request` binds the stack façade before
invoking the method on it:

```rust
// Genere (simplifie)
// Construit une seule fois a l'enregistrement, depuis le graphe de beans :
let core: Arc<UserController> = Arc::new(UserController::from_context(&ctx));

get({
    let core = core.clone();
    move |data: __R2eRequestData_UserController| {
        let core = core.clone(); // un clone d'Arc par requete
        async move {
            // Lie les valeurs request-scoped (identity) dans la façade.
            let ctrl = __r2e_meta_UserController::bind_request(core, data);
            // self.user est un champ de la façade ; self.<inject/config> passe par Deref vers le core.
            ctrl.me().await
        }
    }
})
```

## Validation Criteria

```bash
# Sans token → 401
curl http://localhost:3000/users
# → 401

# Avec token valide → 200
curl -H "Authorization: Bearer <token>" http://localhost:3000/users
# → [...users...]

# Endpoint /me → identite de l'utilisateur
curl -H "Authorization: Bearer <token>" http://localhost:3000/me
# → {"sub":"user-123","email":"demo@r2e.dev","roles":["user","admin"],...}

# Admin sans role admin → 403
TOKEN_USER=$(generate_token_with_roles "user")
curl -H "Authorization: Bearer $TOKEN_USER" http://localhost:3000/admin/users
# → 403

# Admin avec role admin → 200
TOKEN_ADMIN=$(generate_token_with_roles "admin")
curl -H "Authorization: Bearer $TOKEN_ADMIN" http://localhost:3000/admin/users
# → [...users...]
```
