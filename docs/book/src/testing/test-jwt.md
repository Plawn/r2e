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

## Getting the app's `TestJwt`

When you boot an `App` with `#[r2e::test(app = ...)]`, the harness pins a
`TestJwt` over the app's own validator for you. Most tests never touch it
directly — `.as_user(sub, roles)` mints and attaches a token in one call:

```rust
#[r2e::test(app = my_app::MyApp)]
async fn admin_panel(app: TestApp) {
    app.get("/admin/panel")
        .as_user("admin-1", &["admin"])
        .send()
        .await
        .assert_ok();
}
```

Bind an `app: TestApp` **and** a `jwt: TestJwt` parameter when you need the raw
token API — expired tokens, custom claims, or reusing one token across
requests. It is the same key the app validates against:

```rust
#[r2e::test(app = my_app::MyApp)]
async fn custom_claims(app: TestApp, jwt: TestJwt) {
    let token = jwt.token_builder("user-1")
        .roles(&["admin"])
        .claim("tenant_id", "acme-corp")
        .build();

    app.get("/admin/panel")
        .bearer(&token)
        .send()
        .await
        .assert_ok();
}
```

Hand-assembled apps (`TestApp::from_builder`) instead provide the validator
explicitly with `.provide(Arc::new(jwt.claims_validator()))` — see
[Test Setup](./test-setup.md#hand-assembled-apps-testappfrom_builder).

## Testing different roles

```rust
#[r2e::test(app = my_app::MyApp)]
async fn role_access(app: TestApp) {
    // Regular user — forbidden
    app.get("/admin/panel")
        .as_user("user-1", &["user"])
        .send()
        .await
        .assert_forbidden();

    // Admin user — allowed
    app.get("/admin/panel")
        .as_user("admin-1", &["admin"])
        .send()
        .await
        .assert_ok();
}
```

## Testing unauthenticated access

```rust
#[r2e::test(app = my_app::MyApp)]
async fn no_auth(app: TestApp) {
    // No token → 401
    app.get("/users").send().await.assert_unauthorized();
}
```
