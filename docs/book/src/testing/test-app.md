# TestApp

`TestApp` provides an HTTP client for integration testing without starting a TCP server.

## Creating a TestApp

```rust
use r2e_test::TestApp;

let app = TestApp::from_builder(
    AppBuilder::new()
        .build_state::<AppState, _>()
        .await
        .register_controller::<UserController>(),
);
```

## HTTP methods

### Unauthenticated requests

```rust
// GET
let resp = app.get("/users").await;

// POST with JSON
let resp = app.post_json("/users", &serde_json::json!({
    "name": "Alice",
    "email": "alice@example.com"
})).await;
```

### Authenticated requests

```rust
let jwt = TestJwt::new();
let token = jwt.token("user-1", &["user"]);

// GET with auth
let resp = app.get_authenticated("/users", &token).await;

// POST with auth
let resp = app.post_json_authenticated("/users", &body, &token).await;

// PUT with auth
let resp = app.put_json_authenticated("/users/1", &body, &token).await;

// DELETE with auth
let resp = app.delete_authenticated("/users/1", &token).await;
```

## TestResponse

All methods return a `TestResponse` with assertion helpers:

### Status assertions

```rust
resp.assert_ok();           // 200
resp.assert_unauthorized();  // 401
resp.assert_forbidden();     // 403
resp.assert_bad_request();   // 400
resp.assert_status(StatusCode::CREATED);  // custom status
```

### Body access

```rust
// Deserialize JSON
let users: Vec<User> = resp.json();

// Raw text
let body: String = resp.text();
```

### Chaining

Assertions return the response for chaining:

```rust
let users: Vec<User> = app
    .get_authenticated("/users", &token)
    .await
    .assert_ok()
    .json();
```

## Complete example

```rust
#[tokio::test]
async fn test_crud_flow() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["admin"]);

    // List users
    let users: Vec<User> = app
        .get_authenticated("/users", &token)
        .await
        .assert_ok()
        .json();
    assert_eq!(users.len(), 2);

    // Create user
    let new_user: User = app
        .post_json_authenticated("/users", &serde_json::json!({
            "name": "Charlie",
            "email": "charlie@example.com"
        }), &token)
        .await
        .assert_ok()
        .json();
    assert_eq!(new_user.name, "Charlie");

    // Verify creation
    let users: Vec<User> = app
        .get_authenticated("/users", &token)
        .await
        .assert_ok()
        .json();
    assert_eq!(users.len(), 3);
}
```
