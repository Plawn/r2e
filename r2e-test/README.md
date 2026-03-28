# r2e-test

Test utilities for R2E — TestApp HTTP client, TestJwt token generation, TestSession cookie persistence, and assertion helpers.

## Overview

Provides helpers for writing integration tests against R2E applications without starting a real HTTP server. `TestApp` wraps a `Router` and sends requests through Tower's service layer.

## Usage

Add as a dev dependency:

```toml
[dev-dependencies]
r2e-test = "0.1"
```

## Key types

### TestApp

Wraps a `Router` with a builder-based HTTP client for integration testing:

```rust
use r2e_test::TestApp;

#[tokio::test]
async fn test_list_users() {
    let app = build_app().await; // your Router
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

Builder pattern for headers, body, authentication, query params, form data, and cookies:

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

// Query parameters
let response = client
    .get("/search")
    .query("q", "rust")
    .query("page", "1")
    .send()
    .await;

// Form data
let response = client
    .post("/login")
    .form(&[("user", "admin"), ("pass", "secret")])
    .send()
    .await;

// Cookies
let response = client
    .get("/dashboard")
    .cookie("session", "abc123")
    .send()
    .await;
```

Available methods: `get`, `post`, `put`, `delete`, `patch`, `request`.

### TestResponse

Response wrapper with status assertions, JSON-path assertions, JSON matching, and body access.

#### Status assertions

```rust
resp.assert_ok();                    // 200
resp.assert_created();               // 201
resp.assert_no_content();            // 204
resp.assert_bad_request();           // 400
resp.assert_unauthorized();          // 401
resp.assert_forbidden();             // 403
resp.assert_not_found();             // 404
resp.assert_conflict();              // 409
resp.assert_unprocessable();         // 422
resp.assert_too_many_requests();     // 429
resp.assert_internal_server_error(); // 500
resp.assert_status(StatusCode::IM_A_TEAPOT); // any status
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

#### JSON contains (partial matching)

```rust
resp.assert_json_contains(serde_json::json!({
    "name": "Alice",
    "status": "active"
}));
// Passes even if response has extra fields

resp.assert_json_path_contains("users[0]", serde_json::json!({"name": "Alice"}));
```

#### JSON shape validation

```rust
resp.assert_json_shape(serde_json::json!({
    "id": 0,          // expects a number
    "name": "",        // expects a string
    "active": true,    // expects a boolean
    "tags": [""]       // expects array of strings
}));
```

#### Header assertions

```rust
resp.assert_header("content-type", "application/json");
resp.assert_header_exists("x-request-id");
```

#### Cookie and body access

```rust
let session = resp.cookie("session_id");
let all_cookies = resp.cookies();
let content_type = resp.header("content-type");
let users: Vec<User> = resp.json();
let body: String = resp.text();
```

### TestSession

Persists cookies across multiple requests, simulating a browser session:

```rust
let session = app.session();

// Login — cookies from Set-Cookie are captured
session.post("/login")
    .form(&[("user", "admin"), ("pass", "secret")])
    .send()
    .await
    .assert_ok();

// Subsequent requests include captured cookies automatically
session.get("/dashboard").send().await.assert_ok();
```

Configure default headers for the session:

```rust
let session = app.session()
    .with_bearer("my-token")
    .with_default_header("x-tenant-id", "acme");
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

### TestState derive

Eliminates `FromRef` boilerplate for test state structs:

```rust
use r2e::prelude::*;

#[derive(Clone, TestState)]
struct TestState {
    user_service: UserService,
    jwt_validator: Arc<JwtClaimsValidator>,
    config: R2eConfig,
}
// Automatically generates FromRef impls for each field type
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
