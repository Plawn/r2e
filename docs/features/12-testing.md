# Feature 12 — Testing

## Objectif

Fournir des utilitaires de test pour ecrire des tests d'integration in-process sans demarrer de serveur TCP : client HTTP simule (`TestApp`) et generation de JWT de test (`TestJwt`).

## Concepts cles

### TestApp

Client HTTP in-process qui dispatch les requetes via `tower::ServiceExt::oneshot`. Pas de port TCP, pas de reseau — les tests sont rapides et deterministes.

### TestResponse

Wrapper de reponse avec des methodes d'assertion fluides (`assert_ok()`, `assert_not_found()`, etc.).

### TestJwt

Generateur de tokens JWT pour les tests, avec un `JwtValidator` correspondant pre-configure.

## Utilisation

### 1. Ajouter la dependance

```toml
[dev-dependencies]
r2e-test = { path = "../r2e-test" }
http = "1"
```

### 2. Setup de test

```rust
use r2e_core::AppBuilder;
use r2e_core::Controller;
use r2e_test::{TestApp, TestJwt};

async fn setup() -> (TestApp, TestJwt) {
    let jwt = TestJwt::new();

    // Creer l'etat de test
    let services = TestServices {
        user_service: UserService::new(),
        jwt_validator: Arc::new(jwt.validator()),
        pool: SqlitePool::connect("sqlite::memory:").await.unwrap(),
        config: R2eConfig::empty(),
        // ...
    };

    // Construire l'app via AppBuilder
    let app = TestApp::from_builder(
        AppBuilder::new()
            .with_state(services)
            .with_health()
            .with_error_handling()
            .register_controller::<MyController>(),
    );

    (app, jwt)
}
```

### 3. Ecrire des tests

#### Test simple (sans authentification)

```rust
#[tokio::test]
async fn test_health_endpoint() {
    let (app, _jwt) = setup().await;
    let resp = app.get("/health").await.assert_ok();
    assert_eq!(resp.text(), "OK");
}
```

#### Test avec authentification

```rust
#[tokio::test]
async fn test_list_users_authenticated() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    let resp = app.get_authenticated("/users", &token).await.assert_ok();
    let users: Vec<User> = resp.json();
    assert_eq!(users.len(), 2);
}
```

#### Test d'un endpoint protege sans token

```rust
#[tokio::test]
async fn test_list_users_unauthenticated() {
    let (app, _jwt) = setup().await;
    app.get("/users").await.assert_unauthorized();
}
```

#### Test de controle de roles

```rust
#[tokio::test]
async fn test_admin_endpoint_with_admin_role() {
    let (app, jwt) = setup().await;
    let token = jwt.token("admin-1", &["admin"]);
    app.get_authenticated("/admin/users", &token).await.assert_ok();
}

#[tokio::test]
async fn test_admin_endpoint_without_admin_role() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    app.get_authenticated("/admin/users", &token).await.assert_forbidden();
}
```

#### Test POST avec JSON

```rust
#[tokio::test]
async fn test_create_user() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    let body = serde_json::json!({
        "name": "Charlie",
        "email": "charlie@example.com"
    });
    let resp = app.post_json_authenticated("/users", &body, &token)
        .await
        .assert_ok();
    let user: User = resp.json();
    assert_eq!(user.name, "Charlie");
}
```

#### Test de validation (rejet 400)

```rust
#[tokio::test]
async fn test_create_user_with_invalid_email() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    let body = serde_json::json!({
        "name": "Valid Name",
        "email": "not-an-email"
    });
    app.post_json_authenticated("/users", &body, &token)
        .await
        .assert_bad_request();
}
```

#### Test d'un status HTTP specifique

```rust
#[tokio::test]
async fn test_custom_error() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    let resp = app
        .get_authenticated("/error/custom", &token)
        .await
        .assert_status(http::StatusCode::from_u16(418).unwrap());
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"], "I'm a teapot");
}
```

#### Test de rate limiting

```rust
#[tokio::test]
async fn test_rate_limited_endpoint() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    let body = serde_json::json!({"name": "Test", "email": "t@t.com"});

    // Les N premieres requetes passent
    for _ in 0..3 {
        app.post_json_authenticated("/users/rate-limited", &body, &token)
            .await
            .assert_ok();
    }

    // La requete suivante est rejetee
    app.post_json_authenticated("/users/rate-limited", &body, &token)
        .await
        .assert_status(http::StatusCode::TOO_MANY_REQUESTS);
}
```

## API TestApp

### Methodes de requete

| Methode | Description |
|---------|-------------|
| `get(path)` | GET sans authentification |
| `get_authenticated(path, token)` | GET avec Bearer token |
| `post_json(path, body)` | POST avec body JSON |
| `post_json_authenticated(path, body, token)` | POST JSON avec Bearer token |
| `put_json_authenticated(path, body, token)` | PUT JSON avec Bearer token |
| `delete_authenticated(path, token)` | DELETE avec Bearer token |
| `send(request)` | Requete arbitraire (`http::Request<Body>`) |

### Methodes de TestResponse

| Methode | Verifie |
|---------|---------|
| `assert_ok()` | Status 200 |
| `assert_created()` | Status 201 |
| `assert_bad_request()` | Status 400 |
| `assert_unauthorized()` | Status 401 |
| `assert_forbidden()` | Status 403 |
| `assert_not_found()` | Status 404 |
| `assert_status(code)` | Status arbitraire |
| `json::<T>()` | Deserialise le body en `T` |
| `text()` | Body en `String` |

Toutes les methodes `assert_*` retournent `self` pour le chainage :

```rust
let users: Vec<User> = app
    .get_authenticated("/users", &token)
    .await
    .assert_ok()
    .json();
```

## API TestJwt

| Methode | Description |
|---------|-------------|
| `TestJwt::new()` | Cree un generateur avec secret/issuer/audience par defaut |
| `TestJwt::with_config(secret, issuer, audience)` | Cree un generateur avec config custom |
| `token(sub, roles)` | Genere un JWT avec subject et roles |
| `token_with_claims(sub, roles, email)` | Genere un JWT avec email optionnel |
| `validator()` | Retourne un `JwtValidator` qui accepte les tokens generes |

### Tokens generes

Les tokens sont signes en HMAC-SHA256 avec une validite de 1 heure et contiennent :

```json
{
    "sub": "user-1",
    "roles": ["user"],
    "iss": "r2e-test",
    "aud": "r2e-test-app",
    "exp": 1706130000
}
```

## Pattern : controller de test dedie

Pour les tests d'integration, il est courant de redefinir le controller dans le fichier de test (car le crate binaire n'est pas importable) :

```rust
// tests/user_controller_test.rs
use r2e_core::prelude::*;

// Redefinir les types necessaires
mod common {
    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
    pub struct User { pub id: u64, pub name: String, pub email: String }
    // ...
}

// Redefinir le controller de test
#[derive(Controller)]
#[controller(state = TestServices)]
pub struct TestUserController {
    #[inject] user_service: UserService,
    #[identity] user: AuthenticatedUser,
}

#[routes]
impl TestUserController {
    // ... memes routes que le vrai controller
}
```

## Lancer les tests

```bash
# Tous les tests du workspace
cargo test --workspace

# Tests d'un crate specifique
cargo test -p example-app

# Un test specifique
cargo test -p example-app test_health_endpoint
```

## Critere de validation

```bash
cargo test --workspace
# → tous les tests passent (integration + unitaires)
```
