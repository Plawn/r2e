# Test Setup

R2E provides `r2e-test` with utilities for integration testing.

## Dependencies

Add to your `Cargo.toml`:

```toml
[dev-dependencies]
r2e-test = "0.1"
tokio = { version = "1", features = ["full"] }
```

## Basic test structure

```rust
use r2e::prelude::*;
use r2e_test::{TestApp, TestJwt};

async fn setup() -> (TestApp, TestJwt) {
    let jwt = TestJwt::new();

    // Build app state for testing
    let app = TestApp::from_builder(
        AppBuilder::new()
            .provide(Arc::new(jwt.claims_validator()))
            .with_bean::<UserService>()
            .build_state::<AppState, _, _>()
            .await
            .with(Health)
            .with(ErrorHandling)
            .register_controller::<UserController>(),
    );

    (app, jwt)
}

#[tokio::test]
async fn test_health_check() {
    let (app, _) = setup().await;
    app.get("/health").await.assert_ok();
}
```

## Key pattern: `TestApp::from_builder`

`TestApp` wraps your router with an in-process HTTP client (via `tower::ServiceExt::oneshot` — no TCP). This means:

- Tests run fast (no network overhead)
- No port conflicts
- Full request/response lifecycle

```rust
let app = TestApp::from_builder(
    AppBuilder::new()
        // ... same setup as your main.rs, but with test fixtures
        .register_controller::<UserController>(),
);
```

## Test configuration

For tests, use `R2eConfig::empty()` or manual config:

```rust
let config = R2eConfig::empty();
config.set("app.name", ConfigValue::String("test-app".into()));
```

## Running tests

```bash
cargo test --workspace
```
