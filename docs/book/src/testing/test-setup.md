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
    app.get("/health").send().await.assert_ok();
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

## Reducing boilerplate with `#[derive(TestState)]`

Test state structs typically need `FromRef` impls for each field so Axum can extract sub-state. Use `#[derive(TestState)]` to auto-generate these:

```rust
use r2e::prelude::*;

// Before: manual FromRef impls for every field
#[derive(Clone)]
struct TestState {
    user_service: UserService,
    jwt_validator: Arc<JwtClaimsValidator>,
    config: R2eConfig,
}
// impl FromRef<TestState> for UserService { ... }
// impl FromRef<TestState> for Arc<JwtClaimsValidator> { ... }
// impl FromRef<TestState> for R2eConfig { ... }

// After: one derive does it all
#[derive(Clone, TestState)]
struct TestState {
    user_service: UserService,
    jwt_validator: Arc<JwtClaimsValidator>,
    config: R2eConfig,
}
```

Skip fields that shouldn't get a `FromRef` impl:

```rust
#[derive(Clone, TestState)]
struct TestState {
    user_service: UserService,
    #[test_state(skip)]
    internal_counter: Arc<AtomicU64>,
}
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
