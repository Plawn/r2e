# Feature 12 — Testing

## Goal

Provide testing utilities for writing in-process integration tests without starting a TCP server: simulated HTTP client (`TestApp`), test JWT generation (`TestJwt`), session persistence (`TestSession`), and rich assertion helpers.

## Key Concepts

### TestApp

In-process HTTP client that dispatches requests via `tower::ServiceExt::oneshot`. No TCP port, no network — tests are fast and deterministic.

### TestRequest

Builder pattern for constructing requests: headers, body (JSON, form, raw), query parameters, cookies, Bearer tokens.

### TestResponse

Response wrapper with fluent assertion methods (`assert_ok()`, `assert_not_found()`, `assert_json_path()`, `assert_json_contains()`, `assert_json_shape()`, etc.). All assertions return `&Self` for chaining.

### TestSession

Cookie-persisting session that automatically captures `Set-Cookie` headers and sends them on subsequent requests. Useful for login flows and stateful interactions.

### TestJwt

JWT token generator for tests, with a corresponding pre-configured `JwtValidator`.

### TestState derive

`#[derive(TestState)]` auto-generates `FromRef` impls for test state structs, eliminating boilerplate.

## Usage

### 1. Adding the Dependency

```toml
[dev-dependencies]
r2e-test = { path = "../r2e-test" }
```

### 2. Test Setup

```rust
use r2e::prelude::*;
use r2e_test::{TestApp, TestJwt};

#[derive(Clone, TestState)]
struct TestServices {
    user_service: UserService,
    jwt_validator: Arc<JwtClaimsValidator>,
    config: R2eConfig,
}

async fn setup() -> (TestApp, TestJwt) {
    let jwt = TestJwt::new();

    let app = TestApp::from_builder(
        AppBuilder::new()
            .provide(Arc::new(jwt.claims_validator()))
            .with_bean::<UserService>()
            .build_state::<TestServices, _, _>()
            .await
            .with(Health)
            .with(ErrorHandling)
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
    app.get("/health").send().await.assert_ok();
}
```

#### Test with authentication

```rust
#[tokio::test]
async fn test_list_users_authenticated() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);

    let resp = app.get("/users")
        .bearer(&token)
        .send()
        .await;
    resp.assert_ok();
    let users: Vec<User> = resp.json();
    assert_eq!(users.len(), 2);
}
```

#### Test of a protected endpoint without token

```rust
#[tokio::test]
async fn test_list_users_unauthenticated() {
    let (app, _jwt) = setup().await;
    app.get("/users").send().await.assert_unauthorized();
}
```

#### Role-based access control test

```rust
#[tokio::test]
async fn test_admin_endpoint_with_admin_role() {
    let (app, jwt) = setup().await;
    let token = jwt.token("admin-1", &["admin"]);
    app.get("/admin/users").bearer(&token).send().await.assert_ok();
}

#[tokio::test]
async fn test_admin_endpoint_without_admin_role() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    app.get("/admin/users").bearer(&token).send().await.assert_forbidden();
}
```

#### POST test with JSON

```rust
#[tokio::test]
async fn test_create_user() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["admin"]);

    app.post("/users")
        .json(&serde_json::json!({
            "name": "Charlie",
            "email": "charlie@example.com"
        }))
        .bearer(&token)
        .send()
        .await
        .assert_ok()
        .assert_json_path("name", "Charlie");
}
```

#### Query parameter test

```rust
#[tokio::test]
async fn test_search_with_params() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);

    app.get("/users")
        .bearer(&token)
        .query("page", "2")
        .query("size", "10")
        .send()
        .await
        .assert_ok()
        .assert_json_path("meta.page", 2);
}
```

#### Form data test

```rust
#[tokio::test]
async fn test_login_form() {
    let (app, _) = setup().await;
    app.post("/login")
        .form(&[("username", "alice"), ("password", "secret")])
        .send()
        .await
        .assert_ok();
}
```

#### Session test

```rust
#[tokio::test]
async fn test_session_flow() {
    let (app, _) = setup().await;
    let session = app.session();

    session.post("/login")
        .form(&[("username", "alice"), ("password", "secret")])
        .send()
        .await
        .assert_ok();

    // Session cookie is automatically included
    session.get("/dashboard").send().await.assert_ok();
}
```

#### Validation test (400 rejection)

```rust
#[tokio::test]
async fn test_create_user_with_invalid_email() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);

    app.post("/users")
        .json(&serde_json::json!({
            "name": "Valid Name",
            "email": "not-an-email"
        }))
        .bearer(&token)
        .send()
        .await
        .assert_bad_request();
}
```

#### Rate limiting test

```rust
#[tokio::test]
async fn test_rate_limited_endpoint() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);

    for _ in 0..3 {
        app.get("/api/data")
            .bearer(&token)
            .send()
            .await
            .assert_ok();
    }

    app.get("/api/data")
        .bearer(&token)
        .send()
        .await
        .assert_too_many_requests();
}
```

#### JSON shape and partial matching

```rust
#[tokio::test]
async fn test_response_structure() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);

    let resp = app.get("/users/1")
        .bearer(&token)
        .send()
        .await;
    resp.assert_ok();

    // Verify structure without exact values
    resp.assert_json_shape(serde_json::json!({
        "id": 0,
        "name": "",
        "email": ""
    }));

    // Verify subset of values
    resp.assert_json_contains(serde_json::json!({
        "name": "Alice"
    }));
}
```

## TestApp API

### Request Builder Methods

| Method | Description |
|--------|-------------|
| `get(path)` | Start a GET request |
| `post(path)` | Start a POST request |
| `put(path)` | Start a PUT request |
| `patch(path)` | Start a PATCH request |
| `delete(path)` | Start a DELETE request |
| `request(method, path)` | Start a request with any HTTP method |
| `session()` | Create a `TestSession` with cookie persistence |

### TestRequest Builder Methods

| Method | Description |
|--------|-------------|
| `.bearer(token)` | Add Bearer token header |
| `.header(name, value)` | Add a custom header |
| `.json(body)` | Set JSON body (auto-sets Content-Type) |
| `.body(bytes)` | Set raw body |
| `.form(fields)` | Set URL-encoded form body |
| `.cookie(name, value)` | Add a cookie |
| `.query(key, value)` | Add a query parameter |
| `.queries(pairs)` | Add multiple query parameters |
| `.send().await` | Execute the request |

### TestResponse Methods

| Method | Checks |
|--------|--------|
| `assert_ok()` | Status 200 |
| `assert_created()` | Status 201 |
| `assert_no_content()` | Status 204 |
| `assert_bad_request()` | Status 400 |
| `assert_unauthorized()` | Status 401 |
| `assert_forbidden()` | Status 403 |
| `assert_not_found()` | Status 404 |
| `assert_conflict()` | Status 409 |
| `assert_unprocessable()` | Status 422 |
| `assert_too_many_requests()` | Status 429 |
| `assert_internal_server_error()` | Status 500 |
| `assert_status(code)` | Arbitrary status |
| `assert_json_path(path, expected)` | JSON path equals value |
| `assert_json_path_fn(path, predicate)` | JSON path satisfies predicate |
| `assert_json_contains(expected)` | JSON subset match |
| `assert_json_path_contains(path, item)` | JSON path subset match |
| `assert_json_shape(schema)` | Type structure match |
| `assert_header(name, expected)` | Header equals value |
| `assert_header_exists(name)` | Header exists |
| `json::<T>()` | Deserialize body into `T` |
| `json_path::<T>(path)` | Deserialize value at path |
| `text()` | Body as `String` |
| `header(name)` | Get header value |
| `cookie(name)` | Get cookie from Set-Cookie |
| `cookies()` | Get all Set-Cookie values |

All `assert_*` methods return `&Self` for chaining:

```rust
app.get("/users")
    .bearer(&token)
    .send()
    .await
    .assert_ok()
    .assert_json_path("len()", 3)
    .assert_json_shape(serde_json::json!([{"id": 0, "name": ""}]));
```

## TestJwt API

| Method | Description |
|--------|-------------|
| `TestJwt::new()` | Create with default secret/issuer/audience |
| `TestJwt::with_config(secret, issuer, audience)` | Create with custom config |
| `token(sub, roles)` | Generate a JWT with subject and roles |
| `token_with_claims(sub, roles, email)` | Generate a JWT with optional email |
| `token_builder(sub)` | Start a `TokenBuilder` for custom claims |
| `validator()` | Return a `JwtValidator` for these tokens |
| `claims_validator()` | Return a `JwtClaimsValidator` for these tokens |

### TokenBuilder Methods

| Method | Description |
|--------|-------------|
| `.roles(roles)` | Set roles |
| `.email(email)` | Set email claim |
| `.claim(key, value)` | Add a custom claim |
| `.expires_in_secs(secs)` | Set expiration (default: 3600) |
| `.expired()` | Set `exp` to 60 seconds in the past |
| `.build()` | Sign and return the JWT string |

### Generated Tokens

Tokens are signed with HMAC-SHA256 and contain:

```json
{
    "sub": "user-1",
    "roles": ["user"],
    "iss": "r2e-test",
    "aud": "r2e-test-app",
    "exp": 1706130000
}
```

## Running Tests

```bash
# All tests in the workspace
cargo test --workspace

# Tests for a specific crate
cargo test -p example-app

# A specific test
cargo test -p example-app test_health_endpoint
```

## Validation Criteria

```bash
cargo test --workspace
# All tests pass (integration + unit)
```
