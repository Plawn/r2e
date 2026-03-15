# Integration Patterns

Common patterns for writing R2E integration tests.

## Shared setup function

Create a reusable setup that mirrors your production app:

```rust
use r2e::prelude::*;
use r2e_test::{TestApp, TestJwt};
use std::sync::Arc;

#[derive(Clone, TestState)]
struct TestServices {
    jwt_validator: Arc<JwtClaimsValidator>,
    event_bus: LocalEventBus,
    user_service: UserService,
}

async fn setup() -> (TestApp, TestJwt) {
    let jwt = TestJwt::new();
    let event_bus = LocalEventBus::new();

    let app = TestApp::from_builder(
        AppBuilder::new()
            .provide(Arc::new(jwt.claims_validator()))
            .provide(event_bus)
            .with_bean::<UserService>()
            .build_state::<TestServices, _, _>()
            .await
            .with(Health)
            .with(ErrorHandling)
            .register_controller::<UserController>()
            .register_controller::<AccountController>(),
    );

    (app, jwt)
}
```

## Testing validation

```rust
#[tokio::test]
async fn test_validation_errors() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["admin"]);

    // Missing required field
    app.post("/users")
        .json(&serde_json::json!({
            "email": "alice@example.com"
        }))
        .bearer(&token)
        .send()
        .await
        .assert_bad_request();

    // Invalid email
    app.post("/users")
        .json(&serde_json::json!({
            "name": "Alice",
            "email": "not-an-email"
        }))
        .bearer(&token)
        .send()
        .await
        .assert_bad_request();
}
```

## Testing error responses

```rust
#[tokio::test]
async fn test_not_found() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);

    let resp = app.get("/users/99999")
        .bearer(&token)
        .send()
        .await;
    resp.assert_not_found();

    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"], "User not found");
}
```

## Testing response shape

Use `assert_json_shape` to verify the structure without asserting exact values:

```rust
#[tokio::test]
async fn test_user_response_shape() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);

    app.get("/users/1")
        .bearer(&token)
        .send()
        .await
        .assert_ok()
        .assert_json_shape(serde_json::json!({
            "id": 0,
            "name": "",
            "email": "",
            "roles": [""],
            "created_at": ""
        }));
}
```

## Testing partial JSON matching

Use `assert_json_contains` to check a subset of the response:

```rust
#[tokio::test]
async fn test_user_contains_expected_fields() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["admin"]);

    app.post("/users")
        .json(&serde_json::json!({
            "name": "Alice",
            "email": "alice@example.com"
        }))
        .bearer(&token)
        .send()
        .await
        .assert_ok()
        .assert_json_contains(serde_json::json!({
            "name": "Alice",
            "email": "alice@example.com"
        }));
    // Passes even though the response also contains "id", "created_at", etc.
}
```

## Testing with database

For tests with SQLite:

```rust
async fn setup_with_db() -> (TestApp, TestJwt) {
    let jwt = TestJwt::new();
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();

    // Run migrations or create tables
    sqlx::query("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, email TEXT)")
        .execute(&pool)
        .await
        .unwrap();

    // Seed test data
    sqlx::query("INSERT INTO users (name, email) VALUES ('Alice', 'alice@test.com')")
        .execute(&pool)
        .await
        .unwrap();

    let app = TestApp::from_builder(
        AppBuilder::new()
            .provide(Arc::new(jwt.claims_validator()))
            .provide(pool)
            .with_bean::<UserService>()
            .build_state::<AppState, _, _>()
            .await
            .with(ErrorHandling)
            .register_controller::<UserController>(),
    );

    (app, jwt)
}
```

## Testing rate limiting

```rust
#[tokio::test]
async fn test_rate_limiting() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);

    // Make requests up to the limit
    for _ in 0..5 {
        app.get("/api/data")
            .bearer(&token)
            .send()
            .await
            .assert_ok();
    }

    // Next request should be rate limited
    app.get("/api/data")
        .bearer(&token)
        .send()
        .await
        .assert_too_many_requests();
}
```

## Testing with sessions

Use `TestSession` for cookie-based authentication flows:

```rust
#[tokio::test]
async fn test_session_login_flow() {
    let (app, _) = setup().await;
    let session = app.session();

    // Login with form data
    session.post("/login")
        .form(&[("username", "alice"), ("password", "secret")])
        .send()
        .await
        .assert_ok();

    // Session cookie is automatically included
    session.get("/dashboard")
        .send()
        .await
        .assert_ok();
}
```

## Testing with query parameters

```rust
#[tokio::test]
async fn test_pagination() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);

    app.get("/users")
        .bearer(&token)
        .query("page", "2")
        .query("size", "10")
        .send()
        .await
        .assert_ok()
        .assert_json_path("meta.page", 2)
        .assert_json_path("meta.size", 10);
}
```

## Testing events

```rust
#[tokio::test]
async fn test_event_emission() {
    let event_bus = LocalEventBus::new();
    let received = Arc::new(AtomicBool::new(false));
    let received_clone = received.clone();

    event_bus.subscribe(move |_event: Arc<UserCreatedEvent>| {
        let received = received_clone.clone();
        async move {
            received.store(true, Ordering::SeqCst);
        }
    }).await;

    // Setup app with the event bus
    let jwt = TestJwt::new();
    let app = TestApp::from_builder(
        AppBuilder::new()
            .provide(Arc::new(jwt.claims_validator()))
            .provide(event_bus)
            .with_bean::<UserService>()
            .build_state::<AppState, _, _>()
            .await
            .register_controller::<UserController>(),
    );

    let token = jwt.token("admin-1", &["admin"]);
    app.post("/users")
        .json(&serde_json::json!({
            "name": "Alice",
            "email": "alice@test.com"
        }))
        .bearer(&token)
        .send()
        .await
        .assert_ok();

    // Give async event handlers time to run
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(received.load(Ordering::SeqCst));
}
```

## Testing mixed controllers

```rust
#[tokio::test]
async fn test_public_and_protected() {
    let (app, jwt) = setup().await;

    // Public endpoint works without auth
    app.get("/api/public").send().await.assert_ok();

    // Protected endpoint requires auth
    app.get("/api/me").send().await.assert_unauthorized();

    let token = jwt.token("user-1", &["user"]);
    app.get("/api/me")
        .bearer(&token)
        .send()
        .await
        .assert_ok();
}
```
