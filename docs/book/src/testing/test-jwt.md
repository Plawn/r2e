# TestJwt

`TestJwt` generates valid JWT tokens for testing, with configurable claims.

## Creating a TestJwt

```rust
use r2e_test::TestJwt;

let jwt = TestJwt::new();
```

This creates a JWT generator with a random HMAC-SHA256 key. Tokens are valid for 1 hour.

## Generating tokens

### Basic token

```rust
let token = jwt.token("user-123", &["user"]);
```

Creates a token with:
- `sub`: `"user-123"`
- `roles`: `["user"]`
- `iss`, `aud`, `exp`: auto-populated

### Token with custom claims

```rust
let token = jwt.token_with_claims(serde_json::json!({
    "sub": "user-123",
    "email": "alice@example.com",
    "roles": ["user", "admin"],
    "tenant_id": "acme-corp",
    "custom_field": "value",
}));
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
            .build_state::<AppState, _>()
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

    // Regular user
    let user_token = jwt.token("user-1", &["user"]);
    app.get_authenticated("/admin/panel", &user_token)
        .await
        .assert_forbidden();

    // Admin user
    let admin_token = jwt.token("admin-1", &["admin"]);
    app.get_authenticated("/admin/panel", &admin_token)
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
    app.get("/users").await.assert_unauthorized();
}
```
