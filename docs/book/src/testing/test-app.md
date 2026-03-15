# TestApp

`TestApp` provides an HTTP client for integration testing without starting a TCP server.

## Creating a TestApp

```rust
use r2e_test::TestApp;

let app = TestApp::from_builder(
    AppBuilder::new()
        .build_state::<AppState, _, _>()
        .await
        .register_controller::<UserController>(),
);
```

## HTTP methods

All methods return a `TestRequest` builder. Call `.send().await` to execute the request.

### Simple requests

```rust
// GET
let resp = app.get("/users").send().await;

// POST with JSON
let resp = app.post("/users")
    .json(&serde_json::json!({
        "name": "Alice",
        "email": "alice@example.com"
    }))
    .send()
    .await;

// PUT with JSON
let resp = app.put("/users/1")
    .json(&updated_user)
    .send()
    .await;

// PATCH
let resp = app.patch("/users/1")
    .json(&partial_update)
    .send()
    .await;

// DELETE
let resp = app.delete("/users/1").send().await;
```

### Authenticated requests

Use `.bearer()` to add a Bearer token:

```rust
let jwt = TestJwt::new();
let token = jwt.token("user-1", &["user"]);

let resp = app.get("/users").bearer(&token).send().await;

let resp = app.post("/users")
    .json(&body)
    .bearer(&token)
    .send()
    .await;
```

### Custom headers

```rust
let resp = app.get("/users")
    .header("X-Request-Id", "test-123")
    .header("Accept-Language", "fr")
    .bearer(&token)
    .send()
    .await;
```

### Query parameters

```rust
// Single parameter
let resp = app.get("/users")
    .query("page", "1")
    .query("size", "20")
    .send()
    .await;

// Multiple parameters at once
let resp = app.get("/search")
    .queries([("q", "rust"), ("lang", "en")])
    .send()
    .await;

// Works with existing query string in the path
let resp = app.get("/users?active=true")
    .query("page", "1")
    .send()
    .await;
// Result: /users?active=true&page=1
```

### Form data

Send URL-encoded form bodies:

```rust
let resp = app.post("/login")
    .form(&[("username", "alice"), ("password", "secret")])
    .send()
    .await;
```

### Cookies

Add cookies to a request:

```rust
let resp = app.get("/dashboard")
    .cookie("session", "abc123")
    .cookie("theme", "dark")
    .send()
    .await;
```

### Arbitrary method

```rust
use http::Method;

let resp = app.request(Method::OPTIONS, "/users").send().await;
```

## TestResponse

### Status assertions

Common status codes have named helpers:

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

Assertion messages include the response body for easier debugging:

```
Expected 200 OK, got 403 Forbidden
Body: {"error":"Forbidden"}
```

### JSON-path assertions

Assert directly on nested values in the response body using dot-separated paths,
array indices `[N]`, and `len()`/`size()` terminals:

```rust
app.get("/filter/1")
    .bearer(&token)
    .send()
    .await
    .assert_ok()
    .assert_json_path("tagGroups.len()", 2)
    .assert_json_path("tagGroups[0].name", "test Group")
    .assert_json_path("tagGroups[0].tags.len()", 1)
    .assert_json_path("meta.page", 1)
    .assert_json_path("active", true);
```

#### Path syntax

| Path                      | Resolves to                              |
|---------------------------|------------------------------------------|
| `"name"`                  | `root["name"]`                           |
| `"user.email"`            | `root["user"]["email"]`                  |
| `"users[0]"`              | `root["users"][0]`                       |
| `"users[0].name"`         | `root["users"][0]["name"]`               |
| `"users.len()"`           | length of `root["users"]` array          |
| `"groups[0].tags.size()"` | length of `root["groups"][0]["tags"]`     |
| `"meta.len()"`            | number of keys in `root["meta"]` object  |

#### Custom predicates

```rust
resp.assert_json_path_fn("scores", |v| {
    v.as_array().unwrap().iter().all(|s| s.as_f64().unwrap() > 0.0)
});
```

#### Extracting values

```rust
let name: String = resp.json_path("users[0].name");
let count: usize = resp.json_path("items.len()");
```

### JSON contains (partial matching)

Assert that the response JSON contains a subset of keys/values:

```rust
// Object subset matching
resp.assert_json_contains(serde_json::json!({
    "name": "Alice",
    "status": "active"
}));
// Passes even if response has extra fields like "id", "email", etc.

// Array subset matching — each expected element must exist somewhere in the actual array
resp.assert_json_contains(serde_json::json!({
    "tags": ["rust", "api"]
}));
// Passes even if actual tags are ["rust", "web", "api"]

// Partial matching at a specific path
resp.assert_json_path_contains("users[0]", serde_json::json!({"name": "Alice"}));
```

The `json_contains` function is also exported for custom assertions:

```rust
use r2e_test::json_contains;

assert!(json_contains(&actual, &expected));
```

### JSON shape validation

Assert the response matches a type structure without checking exact values.
Schema values serve as type exemplars (`0` = number, `""` = string, `true` = boolean):

```rust
resp.assert_json_shape(serde_json::json!({
    "id": 0,
    "name": "",
    "active": true,
    "tags": [""],
    "profile": {
        "bio": "",
        "verified": true
    }
}));
```

### Header assertions

```rust
resp.assert_header("content-type", "application/json");
resp.assert_header_exists("x-request-id");
```

### Header access

```rust
let content_type = resp.header("content-type");
assert_eq!(content_type, Some("application/json"));
```

### Cookie access

Read cookies from `Set-Cookie` response headers:

```rust
// Get a specific cookie value
let session = resp.cookie("session_id");
assert_eq!(session, Some("abc123".to_string()));

// Get all Set-Cookie header values
let all_cookies = resp.cookies();
```

### Body access

```rust
let users: Vec<User> = resp.json();
let body: String = resp.text();
```

### Chaining

All assertions return `&self` for chaining:

```rust
let users: Vec<User> = app
    .get("/users")
    .bearer(&token)
    .send()
    .await
    .assert_ok()
    .assert_json_path("meta.total", 3)
    .json();
```

> **Note:** Assertions return `&Self` (a reference). If you need to use the response after an assertion, bind the response first:
>
> ```rust
> let resp = app.get("/users").bearer(&token).send().await;
> resp.assert_ok();
> let users: Vec<User> = resp.json();
> ```

## Complete example

```rust
#[tokio::test]
async fn test_crud_flow() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["admin"]);

    // List users
    app.get("/users")
        .bearer(&token)
        .send()
        .await
        .assert_ok()
        .assert_json_path("len()", 2);

    // Create user
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

    // Verify creation
    app.get("/users")
        .bearer(&token)
        .send()
        .await
        .assert_ok()
        .assert_json_path("len()", 3);
}
```
