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
            .register::<UserService>()
            .build_state()
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

## Hand-building test state with `#[derive(TestState)]`

Normally the app state is inferred — `.provide` / `.register` build the bean graph
and `build_state()` materializes it (see [State and Beans](../core-concepts/state-and-beans.md)).
For focused unit-style tests you can instead hand-write a state struct and derive
`TestState` on it. The derive bridges the struct into R2E's HList state model —
generating `HasBean<T, ByField>` / `Contains<T, ByField>` per unique field type (so
controllers can be registered against it and a missing dependency is a compile
error) plus `BeanLookup` (so guards and interceptors can read beans via
`state.bean::<T>()`), along with a per-field `FromRef` impl for any raw Axum
extractors:

```rust
use r2e::prelude::*;

#[derive(Clone, TestState)]
struct TestState {
    user_service: UserService,
    jwt_validator: Arc<JwtClaimsValidator>,
    config: R2eConfig,
}
```

Each field type becomes a bean readable by type — no hand-written `HasBean` /
`BeanLookup` impls required.

Skip fields that shouldn't be exposed as a bean:

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
