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

Wraps an `axum::Router` with a builder-based HTTP client for integration testing:

```rust
use r2e_test::TestApp;

#[tokio::test]
async fn test_list_users() {
    let app = build_app().await; // your axum::Router
    let client = TestApp::new(app);

    let users: Vec<User> = client
        .get("/users")
        .send()
        .await
        .assert_ok()
        .json();
    assert!(!users.is_empty());
}
```

Builder pattern for headers, body, and authentication:

```rust
// POST with JSON body
let response = client
    .post("/users")
    .json(&CreateUser { name: "Alice".into() })
    .header("X-Custom", "value")
    .send()
    .await;

// Authenticated request with Bearer token
let response = client
    .get("/me")
    .bearer(&token)
    .send()
    .await;
```

Available methods: `get`, `post`, `put`, `delete`, `patch`, `request`.

### TestResponse

Response wrapper with status assertions, JSON-path assertions, and body access.

#### Status assertions

```rust
resp.assert_ok();               // 200
resp.assert_created();          // 201
resp.assert_bad_request();      // 400
resp.assert_unauthorized();     // 401
resp.assert_forbidden();        // 403
resp.assert_not_found();        // 404
resp.assert_status(StatusCode::NO_CONTENT); // any status
```

#### JSON-path assertions

Assert directly on nested response body values using dot-separated paths,
array indices, and `len()`/`size()`:

```rust
resp.assert_ok()
    .assert_json_path("users.len()", 2)
    .assert_json_path("users[0].name", "Alice")
    .assert_json_path("users[0].tags.len()", 3)
    .assert_json_path("meta.page", 1)
    .assert_json_path("active", true);
```

For custom predicates:

```rust
resp.assert_json_path_fn("scores", |v| {
    v.as_array().unwrap().iter().all(|s| s.as_f64().unwrap() > 0.0)
});
```

Extract a typed value at a path:

```rust
let name: String = resp.json_path("users[0].name");
let count: usize = resp.json_path("items.len()");
```

#### Header access

```rust
let content_type = resp.header("content-type");
```

#### Body access

```rust
let users: Vec<User> = resp.json();
let body: String = resp.text();
```

### TestJwt

Generates valid JWT tokens for test scenarios:

```rust
use r2e_test::TestJwt;

let jwt = TestJwt::new();

// Simple token
let token = jwt.token("user-123", &["user"]);

// Token with email
let token = jwt.token_with_claims("user-123", &["admin"], Some("alice@example.com"));

// Token builder for custom claims
let token = jwt.token_builder("user-123")
    .roles(&["admin"])
    .email("alice@example.com")
    .claim("tenant_id", "acme-corp")
    .expires_in_secs(7200)
    .build();

// Expired token (for testing rejection)
let expired = jwt.token_builder("user-1")
    .roles(&["user"])
    .expired()
    .build();

// Get validators for wiring into test state
let claims_validator = jwt.claims_validator();
```

## Full example

```rust
use r2e_test::{TestApp, TestJwt};

#[tokio::test]
async fn test_protected_endpoint() {
    let jwt = TestJwt::new();
    let app = build_app_with_validator(jwt.claims_validator()).await;
    let client = TestApp::new(app);
    let token = jwt.token("admin-1", &["admin"]);

    client
        .get("/admin/dashboard")
        .bearer(&token)
        .send()
        .await
        .assert_ok()
        .assert_json_path("users.len()", 3)
        .assert_json_path("users[0].role", "admin");
}
```

## License

Apache-2.0
