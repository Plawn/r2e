# TestSession

`TestSession` persists cookies across multiple requests, simulating a browser session. Useful for testing login flows, CSRF tokens, or any stateful cookie-based interactions.

## Creating a session

```rust
let app = TestApp::from_builder(/* ... */);
let session = app.session();
```

## Basic usage

Cookies from `Set-Cookie` response headers are automatically captured and sent on subsequent requests:

```rust
#[tokio::test]
async fn test_login_flow() {
    let app = build_app().await;
    let session = app.session();

    // Login — server sets a session cookie via Set-Cookie header
    session.post("/login")
        .form(&[("user", "admin"), ("pass", "secret")])
        .send()
        .await
        .assert_ok();

    // Subsequent requests automatically include the session cookie
    session.get("/dashboard")
        .send()
        .await
        .assert_ok();

    // Logout
    session.post("/logout").send().await.assert_ok();

    // Session cookie was cleared by the server
    session.get("/dashboard")
        .send()
        .await
        .assert_unauthorized();
}
```

## Default headers

Set headers that apply to all requests in the session:

```rust
let session = app.session()
    .with_bearer("my-token")
    .with_default_header("x-tenant-id", "acme");

// Both requests include the Bearer token and x-tenant-id header
session.get("/users").send().await.assert_ok();
session.get("/accounts").send().await.assert_ok();
```

Per-request headers override session defaults:

```rust
let session = app.session().with_bearer("default-token");

// This request uses a different token
session.get("/admin")
    .bearer("admin-token")
    .send()
    .await;
```

## Manual cookie management

You can manually manage the session cookie jar:

```rust
let session = app.session();

// Set a cookie manually
session.set_cookie("theme", "dark");
assert_eq!(session.cookie("theme"), Some("dark".to_string()));

// Remove a specific cookie
session.remove_cookie("theme");
assert!(session.cookie("theme").is_none());

// Clear all cookies
session.clear_cookies();
```

## Request builder

`SessionRequest` supports the same builder methods as `TestRequest`:

```rust
session.post("/api/data")
    .json(&payload)
    .bearer("token")
    .header("x-custom", "value")
    .query("page", "1")
    .form(&[("key", "value")])
    .cookie("extra", "cookie")
    .send()
    .await;
```

## HTTP methods

```rust
session.get("/path").send().await;
session.post("/path").send().await;
session.put("/path").send().await;
session.patch("/path").send().await;
session.delete("/path").send().await;
session.request(Method::OPTIONS, "/path").send().await;
```

## Example: CSRF token flow

```rust
#[tokio::test]
async fn test_csrf_protection() {
    let app = build_app().await;
    let session = app.session();

    // GET the form — server sets a CSRF cookie
    let resp = session.get("/form").send().await;
    resp.assert_ok();
    let csrf_token: String = resp.json_path("csrf_token");

    // POST with the CSRF token — session sends the cookie automatically
    session.post("/form")
        .json(&serde_json::json!({
            "csrf_token": csrf_token,
            "data": "value"
        }))
        .send()
        .await
        .assert_ok();
}
```
