# Test Setup

R2E provides `r2e-test` with utilities for integration testing.

## Dependencies

Add to your `Cargo.toml`:

```toml
[dev-dependencies]
r2e-test = "0.1"
tokio = { version = "1", features = ["full"] }
```

## Recommended pattern: boot your app's blueprint

Expose the app's assembly as a **blueprint** function in `lib.rs` (production
`main.rs` boots the same function), then boot the real application in tests:

```rust
// src/lib.rs
pub async fn app(b: AppBuilder) -> impl BootableApp {
    b.load_config::<AppConfig>()
        .register::<UserService>()
        .build_state().await
        .with(Health)
        .with(ErrorHandling)
        .register_controllers::<(UserController,)>()
}

// src/main.rs
#[r2e::main]
async fn main() {
    my_app::app(AppBuilder::new()).await.serve_auto().await.unwrap();
}
```

```rust
// tests/users.rs
use r2e_test::TestApp;

#[r2e::test(app = my_app::app)]
async fn lists_users(app: TestApp) {
    app.get("/users").as_user("alice", &["user"]).send().await.assert_ok();
}
```

Booting an app this way:

- forces the **`test` profile**, so `application-test.yaml` overlays your base
  config,
- pins a local `TestJwt` validator over the app's own, so `.as_user(sub, roles)`
  works with no identity provider,
- retains the resolved bean graph: `app.bean::<UserService>()` (or an
  `#[inject] users: UserService` test parameter) returns the same instance
  your controllers use.

Mocks and config patches go through the `with` hook — overrides are **pinned**,
so the app's own registration of the same type becomes a no-op:

```rust
#[r2e::test(app = my_app::app, with = |b| {
    b.override_bean(FakeMailer::new())          // @InjectMock
        .override_config_value("mail.enabled", false)  // @TestProfile
})]
async fn signup_does_not_send_mail(app: TestApp, #[inject] mailer: FakeMailer) {
    app.post("/signup").json(&payload()).send().await.assert_ok();
    assert_eq!(mailer.sent().len(), 0);
}
```

## Hand-assembled apps: `TestApp::from_builder`

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

Booted apps use the `test` profile: put test-only keys in
`application-test.yaml`, patch individual keys with
`b.override_config_value(key, value)` in the `with` hook, and read the final
config in the test via `app.config()`.

For hand-assembled apps, use `R2eConfig::empty()` or manual config:

```rust
let config = R2eConfig::empty();
config.set("app.name", ConfigValue::String("test-app".into()));
```

## Dev services (containerized infrastructure)

When a test needs real infrastructure, `r2e-devservices` starts Docker
containers on demand (features `postgres`, `redis`):

```rust
use r2e_devservices::DevPostgres;

#[tokio::test]
async fn users_are_persisted() {
    let pg = DevPostgres::shared().await; // one container per test process
    let app = TestApp::boot_with(my_app::app, |b| {
        b.override_config_value("app.database.url", pg.url())
    })
    .await;
    // ...
}
```

`shared()` reuses one container for the whole test process (fast); `start()`
gives an isolated one. Containers are cleaned up after the process exits.

## Running tests

```bash
cargo test --workspace
```
