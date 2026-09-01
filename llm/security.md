---
topic: security
features: security
tokens: ~2200
requires: core-concepts
---

## Security (JWT / OIDC)

### TL;DR

- Enable feature `security` and provide the validator bean
  (`b.provide(Arc::new(JwtClaimsValidator::…))`) — `AuthenticatedUser` resolves
  from it.
- Inject the caller with `#[inject(identity)] user: AuthenticatedUser`; read it
  through `.sub()`, `.email()`, `.roles()`, `.claims()`.
- Mostly-protected controller: put the identity on the **struct** and mark the
  public exceptions `#[anonymous]` (fail-closed). Mostly-public controller: put
  `#[inject(identity)]` on the **handler parameters** instead.
- Reading `self.user` inside an `#[anonymous]` route is a compile error; so are
  `#[anonymous]` + `#[roles]`, and `#[anonymous]` on a controller without a
  required struct identity.
- Use `Option<AuthenticatedUser>` for routes that work with or without auth.
- `#[roles("a", "b")]` is OR, `#[all_roles("a", "b")]` is AND; a failing check
  is 403.
- Custom identity: implement `FromValidatedJwtClaims<S>` **generic over the
  state `S`** (never a concrete state struct), then
  `impl_claims_identity_extractor!(MyIdentity)`.
- Derive `Serialize` but **not** `Deserialize` on an identity type — it must
  never be body-constructible.
- Claims are the typed `StandardClaims`, not a `Value` tree: read known claims
  as fields (`claims.sub`); `claims.get("…")` only reaches the flattened
  `extra` map.
- For your own claim struct, implement `JwtClaimSet` and pass it as the second
  type parameter: `impl_claims_identity_extractor!(TenantUser, claims = TenantClaims)`.

Requires feature: `security`

### Setup

```rust
# fn __doc(b: AppBuilder) -> impl Sized {
use jsonwebtoken::DecodingKey;
use r2e::r2e_security::{JwtClaimsValidator, SecurityConfig};

// Production: JWKS endpoint
let config = SecurityConfig::new("https://idp/.well-known/jwks.json", "issuer", "audience");

// Static key (dev/demo)
let sec_config = SecurityConfig::new("unused", "issuer", "audience")
    .with_allowed_algorithm(jsonwebtoken::Algorithm::HS256);
let validator = JwtClaimsValidator::new_with_static_key(
    DecodingKey::from_secret(b"my-secret"), sec_config);

b.provide(Arc::new(validator))            // the bean AuthenticatedUser resolves
# }
```

### AuthenticatedUser

Implements `Identity` and `FromRequestParts`: extracts + validates the Bearer
JWT. Accessors: `.sub()`, `.email()`, `.roles()`, `.claims()`.

### Struct identity + `#[anonymous]` — fail-closed (PREFERRED for mostly-protected controllers)

A struct-level identity authenticates **every** route; mark public exceptions
with `#[anonymous]` (forgetting the marker fails closed with 401 — safer than
opt-in auth):

```rust
#[controller(path = "/account")]
pub struct AccountController {
    #[inject] service: AccountService,
    #[inject(identity)] user: AuthenticatedUser,   // ALL routes authenticated…
}

#[routes]
impl AccountController {
    #[get("/me")]
    async fn me(&self) -> Json<Profile> {
        Json(self.service.profile(self.user.sub()).await)  // reads self.user
    }

    #[get("/plans")]
    #[anonymous]                                    // …except this one (no JWT cost)
    async fn plans(&self) -> Json<Vec<Plan>> {      // reading self.user here = compile error
        Json(self.service.plans().await)
    }
}
# fn main() {}
```

Rejected at compile time: `#[anonymous]` + `#[roles]`, + a required identity
param, or on a controller without a required struct identity.

### Param-level identity — for mostly-public controllers

```rust
#[controller(path = "/api")]
pub struct MixedController {
    #[inject] user_service: UserService,
}

#[routes]
impl MixedController {
    #[get("/public")]
    async fn public_data(&self) -> Json<Vec<User>> {              // no auth
        Json(self.user_service.list().await)
    }

    #[get("/me")]
    async fn me(&self, #[inject(identity)] user: AuthenticatedUser) -> Json<AuthenticatedUser> {
        Json(user)
    }

    #[get("/maybe")]
    async fn adaptive(&self, #[inject(identity)] user: Option<AuthenticatedUser>) -> Json<Value> {
        Json(json!({ "authenticated": user.is_some() }))
    }
}
# fn main() {}
```

### Role-based access

```rust,ignore
#[get("/admin")]
#[roles("admin")]                 // 403 unless identity has the role
async fn admin_only(&self) -> Json<Data> { ... }

#[roles("admin", "manager")]      // OR — any one suffices
#[all_roles("audit", "finance")]  // AND — all required
```

### Custom identity (database-backed)

Implement `FromValidatedJwtClaims<S>` (generic over the state `S`, never a
concrete state struct), then `impl_claims_identity_extractor!` generates the
extraction glue:

```rust
use r2e::r2e_security::{impl_claims_identity_extractor, AuthenticatedUser, FromValidatedJwtClaims, RoleBasedIdentity};
use r2e::{BeanLookup, Identity, StandardClaims};

#[derive(Clone, Serialize)]                 // NOT Deserialize — never body-constructible
pub struct DbUser { pub auth: AuthenticatedUser, pub profile: UserProfile }

impl Identity for DbUser {
    fn sub(&self) -> &str { self.auth.sub() }
    fn email(&self) -> Option<&str> { self.auth.email() }
    fn claims(&self) -> Option<&StandardClaims> { self.auth.claims() }
}
impl RoleBasedIdentity for DbUser {
    fn roles(&self) -> &[String] { self.auth.roles() }
}

impl<S: BeanLookup + Send + Sync> FromValidatedJwtClaims<S> for DbUser {
    async fn from_jwt_claims(claims: StandardClaims, state: &S) -> Result<Self, HttpError> {
        let auth = AuthenticatedUser::from_claims(claims);
        let pool = state.bean::<SqlitePool>().ok_or_else(|| HttpError::internal("no pool"))?;
        let profile = UserProfile::load(&pool, auth.sub()).await?;   // your own query
        Ok(DbUser { auth, profile })
    }
}
impl_claims_identity_extractor!(DbUser);
```

### Standard claims — `StandardClaims`

JWT claims are **typed**, not a `serde_json::Value` tree. `r2e::StandardClaims`
(in the prelude; also `r2e::r2e_security::StandardClaims`) is deserialized
straight from the token and is what `AuthenticatedUser.claims`,
`Identity::claims()`, `GuardContext::identity_claims()`, `RoleExtractor::extract_roles`
and `IdentityBuilder::build` carry:

```rust
pub struct StandardClaims {
    pub sub: String,                      // "" when absent → rejected as "no sub"
    pub email: Option<String>,
    pub exp: Option<u64>, pub iat: Option<u64>, pub nbf: Option<u64>,
    pub iss: Option<String>,
    pub aud: Option<Audience>,            // string | [string]; .iter() / .contains() / .as_str()
    pub preferred_username: Option<String>, pub name: Option<String>,
    pub scope: Option<String>,            // .scopes() splits it
    pub roles: Option<Vec<String>>,       // plain OIDC
    pub realm_access: Option<RealmAccess>,             // Keycloak; .realm_roles()
    pub resource_access: Option<HashMap<String, ClientAccess>>, // .client_roles("client")
    pub extra: serde_json::Map<String, Value>,         // #[serde(flatten)]: every other claim
}
```

Known claims are fields; anything else is in `extra`, reached with
`claims.get("tenant_id") -> Option<&Value>` (**`get` never returns a known
field** — `claims.get("sub")` is `None`, use `claims.sub`). Custom
`RoleExtractor`s read fields (`claims.realm_roles()`) or walk `extra` with
`r2e::r2e_security::openid::extract_string_array`. The fully dynamic escape
hatch remains: `JwtClaimsValidator::validate_as::<serde_json::Value>` /
`extract_jwt_claims_as::<S, I, Value>` (`Value` still implements `JwtClaimSet`).

### Typed JWT claims — `JwtClaimSet`

By default `from_jwt_claims` receives `StandardClaims`. For your own claim
struct, implement `JwtClaimSet` on a `Deserialize` type and pick it as the
second type parameter — the validated payload deserializes straight into your
type:

```rust
#[derive(Deserialize)]
struct TenantClaims { sub: String, tenant_id: String }

impl JwtClaimSet for TenantClaims {
    fn subject(&self) -> Option<&str> { Some(&self.sub) }
}

#[derive(Clone, Serialize)]                 // NOT Deserialize — never body-constructible
struct TenantUser { sub: String, tenant_id: String }

impl Identity for TenantUser {
    fn sub(&self) -> &str { &self.sub }
}

impl<S: Send + Sync> FromValidatedJwtClaims<S, TenantClaims> for TenantUser {
    async fn from_jwt_claims(claims: TenantClaims, _state: &S) -> Result<Self, HttpError> {
        Ok(TenantUser { sub: claims.sub, tenant_id: claims.tenant_id })
    }
}
impl_claims_identity_extractor!(TenantUser, claims = TenantClaims);
```

The same `Arc<JwtClaimsValidator>` bean validates the JWT once; light
(`AuthenticatedUser`) and custom identities share it.
