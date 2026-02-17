# r2e-test

Test utilities for R2E — TestApp HTTP client and TestJwt token generation.

## Overview

Provides helpers for writing integration tests against R2E applications without starting a real HTTP server. `TestApp` wraps an `axum::Router` and sends requests through Tower's service layer.

## Usage

Add as a dev dependency:

```toml
[dev-dependencies]
r2e-test = "0.1"
```

## Key types

### TestApp

Wraps an `axum::Router` with an HTTP client for integration testing:

```rust
use r2e_test::TestApp;

#[tokio::test]
async fn test_list_users() {
    let app = build_app().await; // your axum::Router
    let client = TestApp::new(app);

    let response = client.get("/users").await;
    assert_eq!(response.status(), 200);

    let users: Vec<User> = response.json().await;
    assert!(!users.is_empty());
}
```

Builder pattern for headers and body:

```rust
let response = client
    .post("/users")
    .json(&CreateUser { name: "Alice".into() })
    .header("X-Custom", "value")
    .await;
```

Available methods: `get`, `post`, `put`, `delete`, `patch`.

### TestResponse

Response wrapper with convenience helpers:

- `status()` — HTTP status code
- `headers()` — response headers
- `text()` — body as string
- `json::<T>()` — deserialize body as JSON

### TestJwt

Generates valid JWT tokens for test scenarios:

```rust
use r2e_test::TestJwt;

let jwt = TestJwt::new()
    .with_sub("user-123")
    .with_email("alice@example.com")
    .with_roles(vec!["admin".into()]);

let token = jwt.token();
let claims_validator = jwt.claims_validator();

// Use in requests
let response = client
    .get("/me")
    .header("Authorization", format!("Bearer {}", token))
    .await;
```

## Full example

```rust
use r2e_test::{TestApp, TestJwt};

#[tokio::test]
async fn test_protected_endpoint() {
    let jwt = TestJwt::new().with_sub("user-1").with_roles(vec!["admin".into()]);
    let app = build_app_with_validator(jwt.claims_validator()).await;
    let client = TestApp::new(app);

    let response = client
        .get("/admin/dashboard")
        .header("Authorization", format!("Bearer {}", jwt.token()))
        .await;

    assert_eq!(response.status(), 200);
}
```

## License

Apache-2.0
