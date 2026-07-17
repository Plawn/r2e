# Test Setup

R2E provides `r2e-test` with utilities for integration testing.

## Dependencies

Add to your `Cargo.toml`:

```toml
[dev-dependencies]
r2e-test = "0.1"
tokio = { version = "1", features = ["full"] }
```

## Recommended pattern: boot your `App`

Implement the `App` trait once in `app.rs`. `lib.rs` compiles that source for
tests; `r2e::app_main!` compiles the same source in the binary tip crate for
production and real hot-reload:

```rust
// src/app.rs
pub struct MyApp;

impl App for MyApp {
    type Env = ();
    async fn setup() {}
    async fn build(b: AppBuilder, _env: ()) -> impl BootableApp {
        b.load_config::<AppConfig>()
            .register::<UserService>()
            .build_state().await
            .with(Health)
            .with(ErrorHandling)
            .register_controllers::<(UserController,)>()
    }
}

// src/lib.rs
include!("app.rs");

// src/main.rs
r2e::app_main!(MyApp);
```

```rust
// tests/users.rs
use r2e_test::TestApp;

#[r2e::test(app = my_app::MyApp)]
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
#[r2e::test(app = my_app::MyApp, with = |b| {
    b.override_bean(FakeMailer::new())          // @InjectMock
        .override_config_value("mail.enabled", false)  // @TestProfile
})]
async fn signup_does_not_send_mail(app: TestApp, #[inject] mailer: FakeMailer) {
    app.post("/signup").json(&payload()).send().await.assert_ok();
    assert_eq!(mailer.sent().len(), 0);
}
```

## Ordered tests (`@Order`)

Most tests should stay independent and parallel. When a scenario genuinely has
to run in sequence — create a resource, then read it back — tag each test with
`order = <u32>`. Tests carrying an `order` run sequentially in ascending order;
tests without one are unaffected and keep running in parallel (no
`--test-threads=1` needed):

```rust
#[r2e::test(app = my_app::MyApp, order = 1)]
async fn creates_user(app: TestApp) {
    app.post("/users").json(&new_user()).send().await.assert_created();
}

#[r2e::test(app = my_app::MyApp, order = 2)]
async fn lists_created_user(app: TestApp) {
    app.get("/users").send().await
        .assert_ok()
        .assert_json_path("/0/name", "Alice");
}
```

Ordering is scoped to the **test binary** (one file under `tests/`) — there is
no cross-binary or cross-crate ordering. Orders need not be contiguous
(`10, 20, 30` is fine). When `app = …` is present, the barrier also covers the
`TestApp` boot, so ordered tests never race on shared dev services.

Add `group = "<name>"` to run several independent sequences in one binary; an
ordered test only waits on lower orders of its **own** group (the default is the
unnamed group):

```rust
#[r2e::test(app = my_app::MyApp, order = 1, group = "billing")]
async fn creates_invoice(app: TestApp) { /* … */ }

#[r2e::test(app = my_app::MyApp, order = 1, group = "catalog")]
async fn seeds_catalog(app: TestApp) { /* … */ } // runs independently of billing
```

Failure semantics are **fail-fast**: if an ordered test fails — by panicking,
or by returning `Err` from a `Result` test — its group is poisoned and every
later test in that group fails immediately with a message naming the failed
predecessor — no deadlock, no cascade of timeouts. A `#[should_panic]` ordered
test that panics as expected is a pass and does **not** poison its group.
Declaring the same `order` twice in one group panics at runtime, naming both
tests.

If a lower order is filtered out (`cargo test <filter>`) or starved by
`--test-threads`, a waiting test would otherwise hang. Instead, once the group
is idle, it panics after a watchdog timeout, listing the pending orders. A
predecessor that is actually running never trips the watchdog, however slow.
Tune the timeout with `R2E_TEST_ORDER_TIMEOUT_SECS` (default `60`).

> `group` without `order` is a compile error, as is `order`/`group` on
> `#[r2e::main]`. Using `order` requires the `r2e-test` dev-dependency (already
> present whenever you use `app = …`).

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
    let pg = DevPostgres::shared().await; // one container for the test session
    let app = TestApp::boot_with::<my_app::MyApp>(|b| {
        b.override_config_value("app.database.url", pg.url())
    })
    .await;
    // ...
}
```

`shared()` reuses one stable container across all test processes in the
workspace session. Each process keeps a TCP lease to a shared Ryuk reaper;
after the final process exits, Ryuk removes the managed containers (10-second
grace by default) and then removes itself. `start()` gives an isolated
handle-scoped container, also labelled so Ryuk can clean it after a crash or
`SIGKILL`.

Ryuk requires a local Docker Unix socket. Override its host path with
`R2E_DEVSERVICES_DOCKER_SOCKET`; use `R2E_DEVSERVICES_KEEP=1` only when the
containers must survive for post-mortem inspection.

## Running tests

```bash
cargo test --workspace
```
