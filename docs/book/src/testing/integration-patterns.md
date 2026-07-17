# Integration Patterns

Common patterns for writing R2E integration tests.

## Boot the real app

Integration tests boot the **same `App` your `main` launches** — never
re-declare controllers or routers in a test. `#[r2e::test(app = ...)]` runs
`App::setup` + `App::build`, forces the `test` profile, and pins a local
`TestJwt` so `.as_user(sub, roles)` mints a valid token with no identity
provider:

```rust
use r2e_test::TestApp;

#[r2e::test(app = my_app::MyApp)]
async fn lists_users(app: TestApp) {
    app.get("/users")
        .as_user("user-1", &["user"])
        .send()
        .await
        .assert_ok();
}
```

See [Test Setup](./test-setup.md) for declaring the `App` and for the `with`
hook that pins mocks (`override_bean`) and patches config
(`override_config_value`).

## Testing validation

```rust
#[r2e::test(app = my_app::MyApp)]
async fn validation_errors(app: TestApp) {
    // Missing required field
    app.post("/users")
        .as_user("user-1", &["admin"])
        .json(&serde_json::json!({
            "email": "alice@example.com"
        }))
        .send()
        .await
        .assert_bad_request();

    // Invalid email
    app.post("/users")
        .as_user("user-1", &["admin"])
        .json(&serde_json::json!({
            "name": "Alice",
            "email": "not-an-email"
        }))
        .send()
        .await
        .assert_bad_request();
}
```

## Testing error responses

```rust
#[r2e::test(app = my_app::MyApp)]
async fn not_found(app: TestApp) {
    let resp = app.get("/users/99999")
        .as_user("user-1", &["user"])
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
#[r2e::test(app = my_app::MyApp)]
async fn user_response_shape(app: TestApp) {
    app.get("/users/1")
        .as_user("user-1", &["user"])
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
#[r2e::test(app = my_app::MyApp)]
async fn user_contains_expected_fields(app: TestApp) {
    app.post("/users")
        .as_user("user-1", &["admin"])
        .json(&serde_json::json!({
            "name": "Alice",
            "email": "alice@example.com"
        }))
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

## Testing with a database

Boot the real app and point it at a throwaway database in the `with` hook —
patch the config key your producer reads, rather than re-wiring a pool by hand:

```rust
#[r2e::test(app = my_app::MyApp, with = |b| {
    b.override_config_value("database.url", "sqlite::memory:")
})]
async fn users_are_listed(app: TestApp, #[inject] pool: sqlx::SqlitePool) {
    // The test shares the exact pool the app resolved.
    sqlx::query("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, email TEXT)")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO users (name, email) VALUES ('Alice', 'alice@test.com')")
        .execute(&pool).await.unwrap();

    app.get("/users")
        .as_user("user-1", &["user"])
        .send()
        .await
        .assert_ok();
}
```

For a real Postgres/Redis container instead of an in-memory database, use
`r2e-devservices` (`DevPostgres::shared()`) via `TestApp::boot_with` — see
[Test Setup](./test-setup.md#dev-services-containerized-infrastructure).

## Testing rate limiting

```rust
#[r2e::test(app = my_app::MyApp)]
async fn rate_limiting(app: TestApp) {
    // Make requests up to the limit
    for _ in 0..5 {
        app.get("/api/data")
            .as_user("user-1", &["user"])
            .send()
            .await
            .assert_ok();
    }

    // Next request should be rate limited
    app.get("/api/data")
        .as_user("user-1", &["user"])
        .send()
        .await
        .assert_too_many_requests();
}
```

## Testing with sessions

Use `TestSession` for cookie-based authentication flows:

```rust
#[r2e::test(app = my_app::MyApp)]
async fn session_login_flow(app: TestApp) {
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
#[r2e::test(app = my_app::MyApp)]
async fn pagination(app: TestApp) {
    app.get("/users")
        .as_user("user-1", &["user"])
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

Read the app's own `EventBus` instance from the resolved graph — an
`#[inject]` test parameter (or `app.bean::<LocalEventBus>()`) returns the same
bean your controllers publish to:

```rust
#[r2e::test(app = my_app::MyApp)]
async fn event_emission(app: TestApp, #[inject] event_bus: LocalEventBus) {
    let received = Arc::new(AtomicBool::new(false));
    let received_clone = received.clone();

    event_bus.subscribe(move |_event: Arc<UserCreatedEvent>| {
        let received = received_clone.clone();
        async move {
            received.store(true, Ordering::SeqCst);
        }
    }).await;

    app.post("/users")
        .as_user("admin-1", &["admin"])
        .json(&serde_json::json!({
            "name": "Alice",
            "email": "alice@test.com"
        }))
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
#[r2e::test(app = my_app::MyApp)]
async fn public_and_protected(app: TestApp) {
    // Public endpoint works without auth
    app.get("/api/public").send().await.assert_ok();

    // Protected endpoint requires auth
    app.get("/api/me").send().await.assert_unauthorized();

    app.get("/api/me")
        .as_user("user-1", &["user"])
        .send()
        .await
        .assert_ok();
}
```
