# Feature 12 — Testing

## Goal

Provide testing utilities for writing in-process integration tests without starting a TCP server: simulated HTTP client (`TestApp`) and test JWT generation (`TestJwt`).

## Key Concepts

### TestApp

In-process HTTP client that dispatches requests via `tower::ServiceExt::oneshot`. No TCP port, no network — tests are fast and deterministic.

### TestResponse

Response wrapper with fluent assertion methods (`assert_ok()`, `assert_not_found()`, etc.).

### TestJwt

JWT token generator for tests, with a corresponding pre-configured `JwtValidator`.

## Usage

### 1. Adding the Dependency

```toml
[dev-dependencies]
r2e-test = { path = "../r2e-test" }
http = "1"
```

### 2. Test Setup

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

### 3. Writing Tests

#### Simple test (without authentication)

```rust
#[tokio::test]
async fn test_health_endpoint() {
    let (app, _jwt) = setup().await;
    let resp = app.get("/health").await.assert_ok();
    assert_eq!(resp.text(), "OK");
}
```

#### Test with authentication

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

#### Test of a protected endpoint without token

```rust
#[tokio::test]
async fn test_list_users_unauthenticated() {
    let (app, _jwt) = setup().await;
    app.get("/users").await.assert_unauthorized();
}
```

#### Role-based access control test

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

#### POST test with JSON

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

#### Validation test (400 rejection)

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

#### Specific HTTP status test

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

#### Rate limiting test

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

## TestApp API

### Request Methods

| Method | Description |
|--------|-------------|
| `get(path)` | GET without authentication |
| `get_authenticated(path, token)` | GET with Bearer token |
| `post_json(path, body)` | POST with JSON body |
| `post_json_authenticated(path, body, token)` | POST JSON with Bearer token |
| `put_json_authenticated(path, body, token)` | PUT JSON with Bearer token |
| `delete_authenticated(path, token)` | DELETE with Bearer token |
| `send(request)` | Arbitrary request (`http::Request<Body>`) |

### TestResponse Methods

| Method | Checks |
|--------|--------|
| `assert_ok()` | Status 200 |
| `assert_created()` | Status 201 |
| `assert_bad_request()` | Status 400 |
| `assert_unauthorized()` | Status 401 |
| `assert_forbidden()` | Status 403 |
| `assert_not_found()` | Status 404 |
| `assert_status(code)` | Arbitrary status |
| `json::<T>()` | Deserialize the body into `T` |
| `text()` | Body as `String` |

All `assert_*` methods return `self` for chaining:

```rust
let users: Vec<User> = app
    .get_authenticated("/users", &token)
    .await
    .assert_ok()
    .json();
```

## TestJwt API

| Method | Description |
|--------|-------------|
| `TestJwt::new()` | Create a generator with default secret/issuer/audience |
| `TestJwt::with_config(secret, issuer, audience)` | Create a generator with custom config |
| `token(sub, roles)` | Generate a JWT with subject and roles |
| `token_with_claims(sub, roles, email)` | Generate a JWT with optional email |
| `validator()` | Return a `JwtValidator` that accepts the generated tokens |

### Generated Tokens

Tokens are signed with HMAC-SHA256, valid for 1 hour, and contain:

```json
{
    "sub": "user-1",
    "roles": ["user"],
    "iss": "r2e-test",
    "aud": "r2e-test-app",
    "exp": 1706130000
}
```

## Pattern: Dedicated Test Controller

For integration tests, it is common to redefine the controller in the test file (since the binary crate cannot be imported):

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

## Running Tests

```bash
# Tous les tests du workspace
cargo test --workspace

# Tests d'un crate specifique
cargo test -p example-app

# Un test specifique
cargo test -p example-app test_health_endpoint
```

## Validation Criteria

```bash
cargo test --workspace
# → tous les tests passent (integration + unitaires)
```
