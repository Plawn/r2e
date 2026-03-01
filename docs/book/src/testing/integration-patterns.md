# Integration Patterns

Common patterns for writing R2E integration tests.

## Shared setup function

Create a reusable setup that mirrors your production app:

```rust
use r2e::prelude::*;
use r2e_test::{TestApp, TestJwt};
use std::sync::Arc;

async fn setup() -> (TestApp, TestJwt) {
    let jwt = TestJwt::new();
    let event_bus = LocalEventBus::new();

    let app = TestApp::from_builder(
        AppBuilder::new()
            .provide(Arc::new(jwt.claims_validator()))
            .provide(event_bus)
            .with_bean::<UserService>()
            .build_state::<AppState, _, _>()
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
    let resp = app.post_json_authenticated("/users", &serde_json::json!({
        "email": "alice@example.com"
    }), &token).await;
    resp.assert_bad_request();

    // Invalid email
    let resp = app.post_json_authenticated("/users", &serde_json::json!({
        "name": "Alice",
        "email": "not-an-email"
    }), &token).await;
    resp.assert_bad_request();
}
```

## Testing error responses

```rust
#[tokio::test]
async fn test_not_found() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);

    let resp = app.get_authenticated("/users/99999", &token).await;
    resp.assert_status(StatusCode::NOT_FOUND);

    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"], "User not found");
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
        app.get_authenticated("/api/data", &token)
            .await
            .assert_ok();
    }

    // Next request should be rate limited
    app.get_authenticated("/api/data", &token)
        .await
        .assert_status(StatusCode::TOO_MANY_REQUESTS);
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
    app.post_json_authenticated("/users", &serde_json::json!({
        "name": "Alice",
        "email": "alice@test.com"
    }), &token).await.assert_ok();

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
    app.get("/api/public").await.assert_ok();

    // Protected endpoint requires auth
    app.get("/api/me").await.assert_unauthorized();

    let token = jwt.token("user-1", &["user"]);
    app.get_authenticated("/api/me", &token).await.assert_ok();
}
```
