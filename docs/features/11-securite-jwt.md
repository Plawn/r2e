# Feature 11 — Securite JWT / Roles

## Objectif

Fournir une authentification JWT complete avec validation de tokens, extraction automatique de l'identite utilisateur, et controle d'acces base sur les roles — le tout integre au systeme de controllers via les attributs `#[identity]` et `#[roles]`.

## Concepts cles

### AuthenticatedUser

Struct representant l'utilisateur authentifie, extraite automatiquement du header `Authorization: Bearer <token>`. Implemente `FromRequestParts` d'Axum.

### JwtValidator

Valide les tokens JWT — supporte les cles statiques (tests, HMAC) et les endpoints JWKS (production, RSA/ECDSA).

### SecurityConfig

Configuration de la validation : URL JWKS, issuer attendu, audience attendue.

### #[identity]

Attribut du `#[derive(Controller)]` qui marque un champ comme extrait de la requete (scope requete). Le champ est automatiquement peuple par l'extracteur `FromRequestParts` correspondant.

### #[roles("...")]

Attribut de methode qui restreint l'acces aux utilisateurs ayant au moins un des roles specifies.

## Utilisation

### 1. Configuration du JwtValidator

#### Mode test (cle statique HMAC)

```rust
use quarlus_security::{JwtValidator, SecurityConfig};
use jsonwebtoken::DecodingKey;

let secret = b"mon-secret-change-en-production";
let config = SecurityConfig::new("unused", "mon-issuer", "mon-audience");
let validator = JwtValidator::new_with_static_key(
    DecodingKey::from_secret(secret),
    config,
);
```

#### Mode production (JWKS endpoint)

```rust
let config = SecurityConfig::new(
    "https://auth.example.com/.well-known/jwks.json",
    "https://auth.example.com",
    "mon-application",
);
let validator = JwtValidator::new(config).await?;
```

Le `JwksCache` telecharge et cache les cles publiques, avec rafraichissement automatique.

### 2. Stocker dans l'etat applicatif

```rust
use std::sync::Arc;

#[derive(Clone)]
pub struct Services {
    pub jwt_validator: Arc<JwtValidator>,
    // ...
}

impl axum::extract::FromRef<Services> for Arc<JwtValidator> {
    fn from_ref(state: &Services) -> Self {
        state.jwt_validator.clone()
    }
}
```

### 3. Utiliser `#[identity]` dans un controller

```rust
use quarlus_security::AuthenticatedUser;

#[derive(quarlus_macros::Controller)]
#[controller(state = Services)]
pub struct UserController {
    #[identity]
    user: AuthenticatedUser,
}

#[quarlus_macros::routes]
impl UserController {
    #[get("/me")]
    async fn me(&self) -> axum::Json<AuthenticatedUser> {
        axum::Json(self.user.clone())
    }
}
```

`AuthenticatedUser` est extrait automatiquement de chaque requete :
- Si le header `Authorization` est absent ou le token invalide → reponse 401
- Si le token est valide → le champ `user` est peuple

### 4. Structure d'AuthenticatedUser

```rust
pub struct AuthenticatedUser {
    pub sub: String,              // Subject (ID utilisateur)
    pub email: Option<String>,    // Email (si present dans les claims)
    pub roles: Vec<String>,       // Roles extraits des claims
    pub claims: serde_json::Value, // Claims bruts pour acces avance
}
```

### Extraction des roles

Les roles sont cherches dans cet ordre :
1. Claim `roles` (tableau de strings)
2. Claim `realm_access.roles` (format Keycloak)

Le trait `RoleExtractor` peut etre implemente pour supporter d'autres formats.

### 5. Controle d'acces avec `#[roles]`

```rust
#[get("/admin/users")]
#[roles("admin")]
async fn admin_list(&self) -> axum::Json<Vec<User>> {
    let users = self.user_service.list().await;
    axum::Json(users)
}
```

Si l'utilisateur n'a pas le role `"admin"`, le handler retourne 403 :

```http
HTTP/1.1 403 Forbidden
Content-Type: application/json

{"error": "Insufficient roles"}
```

#### Roles multiples

```rust
#[roles("admin", "manager")]
```

L'utilisateur doit avoir **au moins un** des roles specifies (OR, pas AND).

### 6. Methodes utilitaires

```rust
// Verifier un role
user.has_role("admin");  // → bool

// Verifier si au moins un role est present
user.has_any_role(&["admin", "manager"]);  // → bool
```

## Flux d'authentification

```
1. Client envoie: Authorization: Bearer eyJhbGciOiJSUzI1NiIs...
2. Extracteur AuthenticatedUser:
   a. Extraire le header Authorization
   b. Verifier le schema "Bearer"
   c. Extraire le token
   d. Valider via JwtValidator (signature, exp, iss, aud)
   e. Mapper les claims vers AuthenticatedUser
3. Si valide → handler execute avec self.user peuple
4. Si invalide → 401 retourne, handler jamais execute
```

### Erreurs possibles

| Cas | Code HTTP | Message |
|-----|-----------|---------|
| Header absent | 401 | Missing Authorization header |
| Schema != Bearer | 401 | Invalid authorization scheme |
| Token invalide | 401 | Invalid token |
| Token expire | 401 | Token expired |
| Issuer/audience invalide | 401 | Token validation failed |

## Code genere par la macro

Pour un controller avec `#[identity] user: AuthenticatedUser`, les macros generent un extractor `__QuarlusExtract_UserController` qui implemente `FromRequestParts` et construit le controller automatiquement :

```rust
// Genere (simplifie)
async fn __quarlus_UserController_me(
    __ctrl_ext: __QuarlusExtract_UserController,  // FromRequestParts — extrait identity + inject + config
) -> axum::Json<AuthenticatedUser> {
    let ctrl = __ctrl_ext.0;
    ctrl.me().await
}
```

## Critere de validation

```bash
# Sans token → 401
curl http://localhost:3000/users
# → 401

# Avec token valide → 200
curl -H "Authorization: Bearer <token>" http://localhost:3000/users
# → [...users...]

# Endpoint /me → identite de l'utilisateur
curl -H "Authorization: Bearer <token>" http://localhost:3000/me
# → {"sub":"user-123","email":"demo@quarlus.dev","roles":["user","admin"],...}

# Admin sans role admin → 403
TOKEN_USER=$(generate_token_with_roles "user")
curl -H "Authorization: Bearer $TOKEN_USER" http://localhost:3000/admin/users
# → 403

# Admin avec role admin → 200
TOKEN_ADMIN=$(generate_token_with_roles "admin")
curl -H "Authorization: Bearer $TOKEN_ADMIN" http://localhost:3000/admin/users
# → [...users...]
```
