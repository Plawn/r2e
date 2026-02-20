# Serveur OIDC embarqué

`r2e-oidc` fournit un serveur OIDC intégré directement dans votre application. Il émet des tokens JWT sans nécessiter de fournisseur d'identité externe (Keycloak, Auth0, etc.). Idéal pour le développement, le prototypage, et les applications monolithiques.

## Installation

Activez la feature `oidc` :

```toml
r2e = { version = "0.1", features = ["security", "oidc"] }
```

## Démarrage rapide

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
    .plugin(oidc)                              // pre-state : fournit Arc<JwtClaimsValidator>
    .build_state::<Services, _, _>().await
    .register_controller::<UserController>()
    .serve("0.0.0.0:3000").await.unwrap();
```

C'est tout. `AuthenticatedUser` fonctionne immédiatement — pas besoin de configurer manuellement un `JwtClaimsValidator`.

## Comment ça marche

`OidcServer` est un `PreStatePlugin`. Lors de l'installation :

1. **Génère une paire de clés RSA-2048** pour signer les tokens
2. **Crée un `JwtClaimsValidator`** avec la clé publique et l'injecte dans le graphe de beans
3. **Enregistre les endpoints OIDC** via une action différée (après la construction du state)

Les tokens émis sont validés localement — pas de requête réseau, pas de cache JWKS.

## Endpoints exposés

| Méthode | Chemin | Description |
|---------|--------|-------------|
| `POST` | `/oauth/token` | Émission de tokens (password / client_credentials) |
| `GET` | `/.well-known/openid-configuration` | Document de découverte OpenID Connect |
| `GET` | `/.well-known/jwks.json` | Clé publique au format JWKS |
| `GET` | `/userinfo` | Informations utilisateur (nécessite Bearer token) |

### Obtenir un token (password grant)

```bash
curl -X POST http://localhost:3000/oauth/token \
  -d "grant_type=password" \
  -d "username=alice" \
  -d "password=password123"
```

Réponse :

```json
{
  "access_token": "eyJhbGciOiJSUzI1NiIs...",
  "token_type": "Bearer",
  "expires_in": 3600
}
```

### Utiliser le token

```bash
curl http://localhost:3000/users/me \
  -H "Authorization: Bearer eyJhbGciOiJSUzI1NiIs..."
```

### Consulter le userinfo

```bash
curl http://localhost:3000/userinfo \
  -H "Authorization: Bearer eyJhbGciOiJSUzI1NiIs..."
```

Réponse :

```json
{
  "sub": "user-1",
  "email": "alice@example.com",
  "roles": ["admin"]
}
```

## Configuration

Le builder offre plusieurs options de personnalisation :

```rust
let oidc = OidcServer::new()
    .issuer("https://myapp.example.com")   // claim `iss` (défaut : "http://localhost:3000")
    .audience("my-app")                     // claim `aud` (défaut : "r2e-app")
    .token_ttl(7200)                        // durée de vie en secondes (défaut : 3600)
    .base_path("/auth")                     // préfixe des endpoints (défaut : "")
    .with_user_store(users);
```

Avec `base_path("/auth")`, les endpoints deviennent :

- `POST /auth/oauth/token`
- `GET /auth/.well-known/openid-configuration`
- `GET /auth/.well-known/jwks.json`
- `GET /auth/userinfo`

## User store

### InMemoryUserStore

Le store en mémoire fourni par défaut, adapté au développement et aux tests :

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

Les mots de passe sont hashés avec **Argon2** — les mots de passe en clair ne sont jamais stockés.

### OidcUser

```rust
pub struct OidcUser {
    pub sub: String,                                    // identifiant unique
    pub email: Option<String>,                          // adresse email
    pub roles: Vec<String>,                             // rôles pour l'autorisation
    pub extra_claims: HashMap<String, serde_json::Value>, // claims supplémentaires
}
```

Les `extra_claims` sont fusionnés dans le JWT. Les claims réservés (`sub`, `iss`, `aud`, `iat`, `exp`, `roles`, `email`) sont ignorés pour éviter les conflits.

### User store personnalisé

Implémentez le trait `UserStore` pour utiliser votre propre backend (SQLx, Redis, LDAP, etc.) :

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

Puis utilisez-le :

```rust
let store = SqlxUserStore { pool: pool.clone() };
let oidc = OidcServer::new().with_user_store(store);
```

## Client credentials grant

Pour les communications service-to-service, configurez un `ClientRegistry` :

```rust
use r2e::r2e_oidc::ClientRegistry;

let clients = ClientRegistry::new()
    .add_client("my-service", "service-secret-key")
    .add_client("batch-worker", "worker-secret");

let oidc = OidcServer::new()
    .with_user_store(users)
    .with_client_registry(clients);
```

Les secrets clients sont aussi hashés avec Argon2.

### Obtenir un token client

```bash
curl -X POST http://localhost:3000/oauth/token \
  -d "grant_type=client_credentials" \
  -d "client_id=my-service" \
  -d "client_secret=service-secret-key"
```

Le token émis a le `client_id` comme `sub` et un tableau `roles` vide.

## Claims JWT

Les tokens émis contiennent les claims suivants :

| Claim | Source | Description |
|-------|--------|-------------|
| `sub` | `OidcUser.sub` / `client_id` | Identifiant unique du sujet |
| `iss` | Configuration | Émetteur du token |
| `aud` | Configuration | Audience cible |
| `iat` | Automatique | Date d'émission (timestamp) |
| `exp` | Configuration | Date d'expiration (timestamp) |
| `roles` | `OidcUser.roles` | Rôles de l'utilisateur |
| `email` | `OidcUser.email` | Email (si défini) |
| *custom* | `OidcUser.extra_claims` | Claims additionnels |

L'algorithme de signature est **RS256** (RSA + SHA-256).

## Gestion des erreurs

Les réponses d'erreur suivent la RFC 6749 (OAuth 2.0) :

```json
{
  "error": "invalid_grant",
  "error_description": "invalid username or password"
}
```

| Code d'erreur | HTTP | Cause |
|--------------|------|-------|
| `invalid_request` | 400 | Paramètre manquant ou invalide |
| `invalid_grant` | 400 | Identifiants invalides (password grant) |
| `unsupported_grant_type` | 400 | Grant type non supporté |
| `invalid_client` | 401 | Identifiants client invalides |
| `unauthorized` | 401 | Token manquant ou invalide (userinfo) |
| `server_error` | 500 | Erreur interne |

## Exemple complet

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
        Json("accessible à tous")
    }

    #[get("/me")]
    async fn me(&self, #[inject(identity)] user: AuthenticatedUser) -> Json<AuthenticatedUser> {
        Json(user)
    }

    #[get("/admin")]
    #[roles("admin")]
    async fn admin(&self, #[inject(identity)] user: AuthenticatedUser) -> Json<&'static str> {
        Json("données admin")
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

## Tests

`r2e-oidc` s'intègre naturellement avec `r2e-test`. Utilisez `OidcServer` dans vos tests d'intégration :

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

// 1. Obtenir un token
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

// 2. Utiliser le token
let resp = client.get("/api/me")
    .header("Authorization", format!("Bearer {access_token}"))
    .await;
assert_eq!(resp.status(), 200);
```

> **Astuce :** Pour les tests simples ne nécessitant pas le flow OAuth complet, `TestJwt` (voir [TestJwt](../testing/test-jwt.md)) reste l'option la plus rapide pour générer des tokens de test.

## Quand utiliser r2e-oidc vs un provider externe

| Scénario | Recommandation |
|----------|---------------|
| Développement local | `r2e-oidc` — aucune infrastructure externe |
| Tests d'intégration | `r2e-oidc` ou `TestJwt` |
| Prototypage / MVP | `r2e-oidc` — déploiement simplifié |
| Application monolithique sans SSO | `r2e-oidc` — gestion des utilisateurs intégrée |
| Production avec SSO | Provider externe (Keycloak, Auth0, etc.) |
| Multi-applications / fédération | Provider externe |

La migration vers un provider externe est transparente : vos contrôleurs utilisent `AuthenticatedUser` dans les deux cas. Seule la configuration dans `main.rs` change.
