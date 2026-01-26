# Etape 4 — quarlus-security : JWT et identite

## Objectif

Implementer le module securite : validation JWT, cache JWKS, et l'extracteur `AuthenticatedUser` compatible avec Axum (`FromRequestParts`).

## Fichiers a creer

```
quarlus-security/src/
  lib.rs              # Re-exports
  identity.rs         # Struct AuthenticatedUser
  jwt.rs              # Validation JWT, decodage claims
  jwks.rs             # Cache JWKS (cles publiques OIDC)
  extractor.rs        # impl FromRequestParts pour AuthenticatedUser
  config.rs           # Configuration securite (issuer, audience, JWKS URL)
```

## 1. Configuration (`config.rs`)

```rust
#[derive(Clone, Debug)]
pub struct SecurityConfig {
    /// URL du endpoint JWKS (ex: https://auth.example.com/.well-known/jwks.json)
    pub jwks_url: String,

    /// Issuer attendu dans le claim "iss"
    pub issuer: String,

    /// Audience attendue dans le claim "aud"
    pub audience: String,

    /// Duree de cache JWKS en secondes (defaut: 3600)
    pub jwks_cache_ttl_secs: u64,
}
```

Le `SecurityConfig` sera stocke dans l'`AppState` pour etre accessible aux extracteurs.

## 2. AuthenticatedUser (`identity.rs`)

```rust
#[derive(Clone, Debug, serde::Serialize)]
pub struct AuthenticatedUser {
    /// Subject (claim "sub")
    pub sub: String,

    /// Email (claim "email", optionnel)
    pub email: Option<String>,

    /// Roles extraits des claims
    pub roles: Vec<String>,

    /// Claims bruts pour acces avance
    pub claims: serde_json::Value,
}
```

### Extraction des roles

Les roles peuvent venir de differents emplacements selon le provider OIDC :

- Keycloak : `realm_access.roles` ou `resource_access.<client>.roles`
- Auth0 : claim custom `https://example.com/roles`
- Generique : claim `roles`

Fournir un trait `RoleExtractor` configurable, avec une implementation par defaut qui cherche dans `roles`, `realm_access.roles`.

## 3. JWKS Cache (`jwks.rs`)

### Responsabilites

1. Telecharger les cles publiques depuis le JWKS endpoint
2. Indexer par `kid` (Key ID)
3. Cacher les cles avec un TTL configurable
4. Rafraichir en arriere-plan quand le TTL expire

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

    /// Recupere la cle de decodage pour un kid donne.
    /// Rafraichit le cache si le kid est inconnu.
    pub async fn get_key(&self, kid: &str) -> Result<DecodingKey, SecurityError> { ... }

    /// Force le rafraichissement du cache.
    async fn refresh(&self) -> Result<(), SecurityError> { ... }
}
```

### Strategie de rafraichissement

1. Si `kid` trouve en cache → retourner directement
2. Si `kid` inconnu → rafraichir le cache puis re-chercher
3. Si toujours inconnu apres refresh → erreur `UnknownKeyId`

## 4. Validation JWT (`jwt.rs`)

```rust
pub struct JwtValidator {
    jwks: Arc<JwksCache>,
    config: SecurityConfig,
}

impl JwtValidator {
    /// Valide un token JWT et retourne un AuthenticatedUser
    pub async fn validate(&self, token: &str) -> Result<AuthenticatedUser, SecurityError> {
        // 1. Decoder le header pour extraire le kid
        // 2. Recuperer la cle depuis le JWKS cache
        // 3. Valider la signature
        // 4. Valider les claims (iss, aud, exp, nbf)
        // 5. Mapper les claims vers AuthenticatedUser
        ...
    }
}
```

### Claims valides

| Claim | Validation |
|-------|-----------|
| `iss` | Doit correspondre a `config.issuer` |
| `aud` | Doit contenir `config.audience` |
| `exp` | Doit etre dans le futur |
| `nbf` | Doit etre dans le passe (si present) |

## 5. Extracteur Axum (`extractor.rs`)

```rust
use axum::extract::FromRequestParts;
use axum::http::request::Parts;

impl<S> FromRequestParts<S> for AuthenticatedUser
where
    S: Send + Sync,
    // Le state doit fournir un JwtValidator
    JwtValidator: axum::extract::FromRef<S>,
{
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        // 1. Extraire le header Authorization
        // 2. Verifier le schema "Bearer"
        // 3. Extraire le token
        // 4. Valider via JwtValidator
        // 5. Retourner AuthenticatedUser ou 401
        ...
    }
}
```

### Extraction du token

```
Authorization: Bearer eyJhbGciOiJSUzI1NiIs...
                      ^^^^^^^^^^^^^^^^^^^^^^^^^
                      token extrait ici
```

### Erreurs possibles

| Cas | Code HTTP | Message |
|-----|-----------|---------|
| Header absent | 401 | Missing Authorization header |
| Schema != Bearer | 401 | Invalid authorization scheme |
| Token invalide | 401 | Invalid token |
| Token expire | 401 | Token expired |
| Kid inconnu | 401 | Unknown signing key |
| Issuer/audience invalide | 401 | Token validation failed |

## 6. Erreurs securite

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

Implementer `IntoResponse` pour `SecurityError` → toutes mappees en 401.

## 7. Integration avec AppState

Pour que l'extracteur fonctionne, le `JwtValidator` doit etre accessible depuis l'etat Axum. Deux approches :

**Approche A** — `FromRef` : l'utilisateur implemente `FromRef<AppState<T>>` pour `JwtValidator`

**Approche B (recommandee)** — Extension Axum : stocker le `JwtValidator` dans les extensions du `Router` via une layer Tower.

## Critere de validation

Test unitaire avec un JWT signe localement (cle RSA generee en test) :

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

## Dependances entre etapes

- Requiert : etape 0, etape 1 (AppError, AppState)
- Bloque : etape 5 (example-app, pour l'integration complete)
- Peut etre fait en parallele de l'etape 2 et 3
