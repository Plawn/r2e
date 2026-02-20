# TestJwt

`TestJwt` generates valid JWT tokens for testing, with configurable claims.

## Creating a TestJwt

```rust
use r2e_test::TestJwt;

let jwt = TestJwt::new();
```

This creates a JWT generator with a static HMAC-SHA256 key. Tokens are valid for 1 hour by default.

## Generating tokens

### Basic token

```rust
let token = jwt.token("user-123", &["user"]);
```

Creates a token with:
- `sub`: `"user-123"`
- `roles`: `["user"]`
- `iss`, `aud`, `exp`: auto-populated

### Token with email

```rust
let token = jwt.token_with_claims("user-123", &["user"], Some("alice@example.com"));
```

### Token builder

For fine-grained control over claims and expiration, use `token_builder()`:

```rust
let token = jwt.token_builder("user-123")
    .roles(&["admin", "user"])
    .email("alice@example.com")
    .claim("tenant_id", "acme-corp")
    .claim("department", "engineering")
    .expires_in_secs(7200)  // 2 hours
    .build();
```

### Expired tokens

Test that your application rejects expired tokens:

```rust
let expired_token = jwt.token_builder("user-1")
    .roles(&["user"])
    .expired()
    .build();

app.get("/users")
    .bearer(&expired_token)
    .send()
    .await
    .assert_unauthorized();
```

### Custom claims

Add any extra claim to the token:

```rust
let token = jwt.token_builder("user-1")
    .roles(&["user"])
    .claim("tenant_id", "acme-corp")
    .claim("org_id", 42)
    .claim("feature_flags", serde_json::json!(["beta", "dark-mode"]))
    .build();
```

## Getting the validator

`TestJwt` provides validators compatible with R2E's security system:

```rust
// For JwtValidator (higher-level)
let validator = jwt.validator();

// For JwtClaimsValidator (low-level, used by AuthenticatedUser)
let claims_validator = jwt.claims_validator();
```

## Wiring into test state

```rust
use std::sync::Arc;

async fn setup() -> (TestApp, TestJwt) {
    let jwt = TestJwt::new();

    let app = TestApp::from_builder(
        AppBuilder::new()
            .provide(Arc::new(jwt.claims_validator()))
            // ...
            .build_state::<AppState, _, _>()
            .await
            .register_controller::<UserController>(),
    );

    (app, jwt)
}
```

## Testing different roles

```rust
#[tokio::test]
async fn test_role_access() {
    let (app, jwt) = setup().await;

    // Regular user — forbidden
    let user_token = jwt.token("user-1", &["user"]);
    app.get("/admin/panel")
        .bearer(&user_token)
        .send()
        .await
        .assert_forbidden();

    // Admin user — allowed
    let admin_token = jwt.token("admin-1", &["admin"]);
    app.get("/admin/panel")
        .bearer(&admin_token)
        .send()
        .await
        .assert_ok();
}
```

## Testing unauthenticated access

```rust
#[tokio::test]
async fn test_no_auth() {
    let (app, _) = setup().await;

    // No token → 401
    app.get("/users").send().await.assert_unauthorized();
}
```
