# Feature 12 — Testing

## TL;DR

In-process integration tests with no TCP server — requests dispatch via `tower::ServiceExt::oneshot`, so tests are fast and deterministic. Boot the **real** app by type: `#[r2e::test(app = my_app::MyApp)]` gives a `TestApp` client plus `#[inject]` bean params; it forces the `test` profile (`application-test.yaml`) and pins a local `TestJwt` validator so `.as_user("alice", &["user"])` needs no IdP. Fluent assertions (`assert_ok()`, `assert_json_path()`, `assert_json_contains()`, …), `TestSession` for cookie flows, mocks/config patches via the `with` hook.


## Goal

Provide testing utilities for writing in-process integration tests without starting a TCP server: simulated HTTP client (`TestApp`), test JWT generation (`TestJwt`), session persistence (`TestSession`), and rich assertion helpers.

## Key Concepts

### TestApp

In-process HTTP client that dispatches requests via `tower::ServiceExt::oneshot`. No TCP port, no network — tests are fast and deterministic.

### TestRequest

Builder pattern for constructing requests: headers, body (JSON, form, raw), query parameters, cookies, Bearer tokens.

### TestResponse

Response wrapper with fluent assertion methods (`assert_ok()`, `assert_not_found()`, `assert_json_path()`, `assert_json_contains()`, `assert_json_shape()`, etc.). All assertions return `&Self` for chaining.

### TestSession

Cookie-persisting session that automatically captures `Set-Cookie` headers and sends them on subsequent requests. Useful for login flows and stateful interactions.

### TestJwt

JWT token generator for tests, with a corresponding pre-configured `JwtValidator`.

### Test state

Tests build their state exactly like production: `.provide(...)` /
`.register::<T>()` on an `AppBuilder`, then `.build_state().await`. The state
is the inferred HList of provided beans — there is no hand-written test state
struct to maintain.

### App boot (recommended)

Instead of hand-assembling a builder per test file, implement the `App` trait
once in `src/app.rs`, include it from `lib.rs`, and boot the **real** application
by type. `r2e::app_main!` compiles the same source into the production binary:

```rust
use r2e_test::TestApp;

#[r2e::test(app = my_app::MyApp)]
async fn lists_users(app: TestApp, #[inject] users: UserService) {
    app.get("/users").as_user("alice", &["user"]).send().await.assert_ok();
    assert_eq!(users.count().await, 2);
}
```

Booting forces the `test` profile (`application-test.yaml` overlays the base
config), pins a local `TestJwt` validator over the app's own (so `.as_user`
needs no IdP), and retains the bean graph (`app.bean::<T>()`, `#[inject]`
test parameters). Mocks and config patches go through the `with` hook:
`#[r2e::test(app = my_app::MyApp, with = |b| b.override_bean(FakeMailer::new()))]`.
Non-macro forms: `TestApp::boot::<my_app::MyApp>()`, `TestApp::boot_with`,
`TestApp::boot_plain` (and the `*_env` variants below). See `examples/example-app/tests/app_test.rs` for the
full showcase.

A boot failure fails **one test**, it does not kill the runner: `App::setup`,
`App::build`, config loading, every bean constructor, and the controller
`#[post_construct]` / `#[on_start]` hooks are fallible, and the boot methods
panic with `TestApp::boot::<MyApp>() failed: <error>` plus the `caused by:`
chain, which libtest attributes to the calling test. (Corollary: never call
`std::process::exit` in `setup`/`build` — that code is linked into the test
binary.) When the failure itself is the subject, `TestApp::try_boot::<A>()` /
`try_boot_with` / `try_boot_plain` return `Result<TestApp, BootError>` over that
same set of steps.

### Sharing one `App::Env` across boots

`App::Env` is the production concept of "resources built once" — a pool, a
migrated schema, a container. Every plain `boot*` calls `A::setup()` again, so a
binary that boots per test replays that work per test. The `*_env` boots take an
environment the caller already owns and hand it straight to `A::build`;
`A::setup()` is not called:

```rust
use r2e::rt::sync::OnceCell;
use r2e_core::App;

static ENV: OnceCell<<MyApp as App>::Env> = OnceCell::const_new();

async fn env() -> <MyApp as App>::Env {
    ENV.get_or_init(|| async { MyApp::setup().await.expect("setup") })
        .await
        .clone()
}

#[r2e::test(app = my_app::MyApp, env = env().await)]
async fn lists_users(app: TestApp) {
    app.get("/users").send().await.assert_ok();
}
```

- **Macro knob:** `env = <expr>` on `#[r2e::test(app = …)]` and on
  `#[r2e::test_suite(app = …)]`. The expression is evaluated in the test's async
  block (so `env().await` is fine) and must produce `<App as App>::Env`. It
  composes with `with = …` and `jwt = false`, and requires `app = …`.
- **Explicit forms**, mirroring `boot` / `boot_with` / `boot_plain`:
  `TestApp::boot_env::<A>(env)`, `TestApp::boot_with_env::<A>(env, |b| …)`,
  `TestApp::boot_plain_env::<A>(env, |b| …)` — plus `try_boot_env`,
  `try_boot_with_env`, `try_boot_plain_env` returning `Result<TestApp,
  BootError>`.
- Everything else is unchanged: `test` profile, pinned `TestJwt` validators, the
  production startup phase, `shutdown()`. Only `setup` is skipped, so a
  `build`-phase failure is still reported exactly as before.
- `#[r2e::test_suite]`'s `#[before_all]` amortises setup **within one suite**;
  a shared `Env` amortises it across suites and across whole test binaries.

**Isolation is your job.** A shared `Env` means shared state: the same pool,
rows and caches for every test booted off it, and the boots still run
concurrently under libtest. Keep the tests independent (per-test schema, prefix
or tenant, unique fixtures) or serialise them with `order = …`. The shared value
also outlives each `TestApp` — `shutdown()` disposes the app's own beans, never
the `Env` a later boot still needs, so dispose it yourself (or let the process
exit do it).

### Ordered tests (@Order)

Keep tests independent and parallel by default. For the occasional scenario that
must run in sequence — create a resource, then read it back — tag each test with
`order = <u32>`. Ordered tests run one after another in ascending order; tests
without an `order` are completely unaffected and stay parallel (no
`--test-threads=1`):

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

- **Scope is the test binary** (one file under `tests/`) — there is no
  cross-binary or cross-crate ordering. Orders need not be contiguous
  (`10, 20, 30` is fine). The registry is filled at binary load via `inventory`,
  and each ordered test waits (a barrier in `r2e-test`) for all lower
  **registered** orders of its group.
- **Works with or without `app = …`.** When `app` is present the barrier covers
  the `TestApp` boot too, so ordered tests never race on shared dev services.
- **Groups:** `group = "<name>"` gives several independent sequences in one
  binary — a test waits only on lower orders of its *own* group. The default is
  the unnamed group.
- **Fail-fast:** if an ordered test fails — panic, or `Err` from a `Result`
  test — its group is poisoned and later tests in that group fail immediately
  with a message naming the failed predecessor — no deadlock. A
  `#[should_panic]` ordered test that panics as expected is a pass and does not
  poison its group.
- **Duplicate `order` in a group** panics at runtime, naming both tests (the
  macro can't see sibling items, so this can't be a compile error).
- **Watchdog:** a waiting test panics instead of hanging if some lower order was
  never started and the group stays idle for `R2E_TEST_ORDER_TIMEOUT_SECS`
  (default `60`) — typically a lower order filtered out by `cargo test <filter>`
  or starved by `--test-threads`. A running predecessor never trips the
  watchdog, however slow. The diagnostic lists the pending orders and whether
  they ever started.
- **Compile errors:** `group` without `order`; `order`/`group` on
  `#[r2e::main]`. Using `order` requires the `r2e-test` dev-dependency (already
  present whenever you use `app = …`).

## Lifecycle: what a test boot runs

`TestApp::boot` is a **real startup**, not a router build. It runs the same
startup phase `serve()` does:

1. controller `#[post_construct]` hooks,
2. consumer registrations (`#[consumer]` methods, subscriber beans, EventBus
   bridges),
3. bean and controller `#[on_start]` hooks,
4. the builder's `.on_start(…)` closures — which is what starts
   `spawn_service` / `#[derive(BackgroundService)]` tasks.

An error from any of them fails the test (`try_boot*` returns it instead).

`app.shutdown().await` runs the matching shutdown sequence, under the app's own
budgets:

```rust
#[r2e::test(app = my_app::MyApp)]
async fn flushes_on_shutdown(app: TestApp) {
    app.post("/orders").json(&order).send().await.assert_created();

    app.shutdown().await;

    // #[pre_destroy] disposers and .on_stop(…) hooks have run by here.
}
```

1. the builder's `.on_drain(…)` hooks, while still serving;
2. plugin shutdown hooks and `#[pre_destroy]` disposers (controller hooks
   first, then bean hooks, each in reverse registration order);
3. the app shutdown token is cancelled and the tracked handles are joined under
   `shutdown_grace_period` — in-flight HTTP requests drain under
   `drain_timeout`, and a server from `app.serve()` drains with them;
4. the builder's `.on_stop(…)` hooks, outside every budget.

### `shutdown()` is the signal path

It deliberately does **not** call `StopHandle::stop()`. Production does not
either: `run()`'s shutdown future is
`select!(shutdown_signal(), stop_handle.stopped())`, so on SIGTERM/SIGINT the
handle is never fired and `StopHandle::is_stopped()` reads `false` for the whole
sequence. A default `app.shutdown().await` therefore reproduces what an
orchestrator's TERM does, and a hook or service that reads the handle behaves
identically in both.

The *programmatic* path — what an admin `/shutdown` endpoint triggers — is one
line away, and now distinguishable from the signal one:

```rust
app.stop_handle().stop();   // is_stopped() == true from here on
app.shutdown().await;       // then the sequence that stop would have started
```

Readiness flips only if the app's own `on_drain` hook flips it. `StopHandle` is
a stop *trigger*, not a readiness switch; nothing in R2E changes a health probe
on its own.

### Dropping instead of shutting down

`shutdown()` is explicit because `Drop` cannot await. Dropping a `TestApp`
cancels the app token and then **aborts** every still-running tracked task —
background services, an attached `app.serve()` server. Nothing would ever join
those handles once the value is gone, so dropping them alone would *detach* the
tasks: a service that ignores cancellation, or that cleans up slowly after
seeing it, would keep running against a graph the test believes is released.
Cancellation is issued before the abort, so a cooperative task may still finish;
a task whose cleanup must *complete* needs `shutdown().await`, which joins under
`shutdown_grace_period`. A drop with work pending logs a warning — a generic
one: `RunningApp` is type-erased and does not know the app's name.

`app.has_shutdown_work()` is `false` only when dropping loses nothing: no
`on_drain`/`#[pre_destroy]`/`on_stop` hook, no unfired plugin sync hook, and no
live tracked task. That is the only case where skipping `shutdown()` is
equivalent to calling it. It is not free in the strict sense — a start allocates
three `Arc`s for the shutdown token, the plugin hook cell and the handle
collector — but no hook runs and no task is spawned.

**What a test boot skips:** the plugin *serve hooks*, which bind ports
(separate-port gRPC, MCP) and start the scheduler driver. So `#[scheduled]`
tasks do not tick under `TestApp`, and WebSocket sessions run untracked
(`ws.shutdown_token()` is `None`). Also skipped, because they *are* the
listener: SO_REUSEPORT sharded serving and QUIC/HTTP3. An in-process start
spawns no worker runtimes, so a registered `per_worker_service()` is refused at
boot rather than silently never started — and an invalid `server.workers` fails
the boot exactly as it does in production.

### Suite tests (`#[r2e::test_suite]`)

When several tests share the same expensive setup, put them on an inherent
`impl` block: `#[before_all]` runs once and builds the suite value, each
`#[case]` becomes its own Cargo test running against that shared value, and
`#[before_each]` / `#[after_each]` / `#[after_all]` bracket them.

```rust
struct UserSuite { app: TestApp, token: String }

#[r2e::test_suite(app = my_app::MyApp)]
impl UserSuite {
    #[before_all]
    async fn setup(app: TestApp, jwt: TestJwt) -> Self {
        Self { token: jwt.token("alice", &["user"]), app }
    }

    #[case]
    async fn creates_user(&mut self) {
        self.app.post("/users").bearer(self.token.clone()).send().await.assert_created();
    }

    #[case]
    async fn lists_users(&mut self) {
        self.app.get("/users").bearer(self.token.clone()).send().await.assert_ok();
    }

    #[after_all]
    async fn teardown(&mut self) { /* full-suite runs only */ }
}
```

- **One runtime for the whole suite.** Every hook and every case run on the same
  reactor, and it stays alive until the suite ends. That is what lets
  `#[before_all]` hold runtime-bound resources — a `TestApp`, a database pool, a
  listening socket, a spawned worker, a timer — and have case 2, case 3 and
  `#[after_all]` still find them working. (A resource whose reactor is gone does
  not error, it stops waking, so this used to surface as an unrelated timeout.)
- **The suite is torn down by its last case.** After `#[after_all]`, the suite
  value is dropped *on* the suite runtime and the runtime is then shut down, so
  a suite's threads and detached tasks do not keep running for the rest of the
  test binary. Anything that reached the suite afterwards would panic naming
  the suite, not hang.
- The runtime knobs go on the attribute and configure that one runtime:
  `#[r2e::test_suite(flavor = "current_thread", worker_threads = 2,
  start_paused = true)]`. With `start_paused` the paused clock is shared by the
  whole suite rather than reset per case.
- `#[before_all]` is optional (the suite type then needs `Default`) and may
  return `Self` or `Result<Self, E>`; it binds `TestApp`, `TestJwt` and
  `#[inject]` beans exactly like `#[r2e::test]`, with the same `app = …`,
  `with = …` and `jwt = false` arguments.
- Cases are unordered by default (access to the suite value is serialized);
  `#[case(order = N)]` opts into the ordered-test barrier above, within that
  suite.
- `#[after_all]` (and the teardown that follows it) runs when the **last
  generated case completes** — counted against the `#[case]`s the macro emitted,
  because libtest does not expose which tests the process selected. A partial
  `cargo test <filter>` run therefore does not run `#[after_all]` at all and
  leaks the suite to process exit; that is a known limitation, not a bug to
  work around in the case body.
- For the same reason **`#[ignore]` on a `#[case]` is a compile error**: an
  ignored case would either suppress teardown (plain run) or let teardown fire
  before it runs (`cargo test -- --include-ignored`). Skip inside the case body,
  or move the test out of the suite.

## Usage

### 1. Adding the Dependency

```toml
[dev-dependencies]
r2e-test = { path = "../r2e-test" }
```

### 2. Test Setup

```rust
use r2e::prelude::*;
use r2e_test::{TestApp, TestJwt};

async fn setup() -> (TestApp, TestJwt) {
    let jwt = TestJwt::new();

    let app = TestApp::from_builder(
        AppBuilder::new()
            .provide(Arc::new(jwt.claims_validator()))
            .register::<UserService>()
            .plugin(Health)
            .plugin(ErrorHandling)
            .build_state()
            .await
            .register_controller::<MyController>(),
    );

    (app, jwt)
}
```

`.build_state()` takes no type arguments — the test state is inferred from
what you `.provide()` / `.register()`, just like in production. Register
several controllers at once with
`.register_controllers::<(A, B, C)>()`.

### 3. Writing Tests

#### Simple test (without authentication)

```rust
#[r2e::test]
async fn test_health_endpoint() {
    let (app, _jwt) = setup().await;
    app.get("/health").send().await.assert_ok();
}
```

#### Test with authentication

```rust
#[r2e::test]
async fn test_list_users_authenticated() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);

    let resp = app.get("/users")
        .bearer(&token)
        .send()
        .await;
    resp.assert_ok();
    let users: Vec<User> = resp.json();
    assert_eq!(users.len(), 2);
}
```

#### Test of a protected endpoint without token

```rust
#[r2e::test]
async fn test_list_users_unauthenticated() {
    let (app, _jwt) = setup().await;
    app.get("/users").send().await.assert_unauthorized();
}
```

#### Role-based access control test

```rust
#[r2e::test]
async fn test_admin_endpoint_with_admin_role() {
    let (app, jwt) = setup().await;
    let token = jwt.token("admin-1", &["admin"]);
    app.get("/admin/users").bearer(&token).send().await.assert_ok();
}

#[r2e::test]
async fn test_admin_endpoint_without_admin_role() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    app.get("/admin/users").bearer(&token).send().await.assert_forbidden();
}
```

#### POST test with JSON

```rust
#[r2e::test]
async fn test_create_user() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["admin"]);

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
}
```

#### Query parameter test

```rust
#[r2e::test]
async fn test_search_with_params() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);

    app.get("/users")
        .bearer(&token)
        .query("page", "2")
        .query("size", "10")
        .send()
        .await
        .assert_ok()
        .assert_json_path("meta.page", 2);
}
```

#### Form data test

```rust
#[r2e::test]
async fn test_login_form() {
    let (app, _) = setup().await;
    app.post("/login")
        .form(&[("username", "alice"), ("password", "secret")])
        .send()
        .await
        .assert_ok();
}
```

#### Session test

```rust
#[r2e::test]
async fn test_session_flow() {
    let (app, _) = setup().await;
    let session = app.session();

    session.post("/login")
        .form(&[("username", "alice"), ("password", "secret")])
        .send()
        .await
        .assert_ok();

    // Session cookie is automatically included
    session.get("/dashboard").send().await.assert_ok();
}
```

#### Validation test (400 rejection)

```rust
#[r2e::test]
async fn test_create_user_with_invalid_email() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);

    app.post("/users")
        .json(&serde_json::json!({
            "name": "Valid Name",
            "email": "not-an-email"
        }))
        .bearer(&token)
        .send()
        .await
        .assert_bad_request();
}
```

#### Rate limiting test

```rust
#[r2e::test]
async fn test_rate_limited_endpoint() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);

    for _ in 0..3 {
        app.get("/api/data")
            .bearer(&token)
            .send()
            .await
            .assert_ok();
    }

    app.get("/api/data")
        .bearer(&token)
        .send()
        .await
        .assert_too_many_requests();
}
```

#### JSON shape and partial matching

```rust
#[r2e::test]
async fn test_response_structure() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);

    let resp = app.get("/users/1")
        .bearer(&token)
        .send()
        .await;
    resp.assert_ok();

    // Verify structure without exact values
    resp.assert_json_shape(serde_json::json!({
        "id": 0,
        "name": "",
        "email": ""
    }));

    // Verify subset of values
    resp.assert_json_contains(serde_json::json!({
        "name": "Alice"
    }));
}
```

## TestApp API

### Request Builder Methods

| Method | Description |
|--------|-------------|
| `get(path)` | Start a GET request |
| `post(path)` | Start a POST request |
| `put(path)` | Start a PUT request |
| `patch(path)` | Start a PATCH request |
| `delete(path)` | Start a DELETE request |
| `request(method, path)` | Start a request with any HTTP method |
| `session()` | Create a `TestSession` with cookie persistence |
| `serve()` | Spawn a live `TestServer` on a random port (attached to a booted app's lifecycle) |
| `shutdown()` | Run the production shutdown sequence (`on_drain` → disposers → drain → join → `on_stop`) |
| `has_shutdown_work()` | Whether any shutdown hook is registered |

### TestRequest Builder Methods

| Method | Description |
|--------|-------------|
| `.bearer(token)` | Add Bearer token header |
| `.as_user(sub, roles)` | Mint a `TestJwt` token for `sub`/`roles` and add it as the Bearer header |
| `.header(name, value)` | Add a custom header |
| `.json(body)` | Set JSON body (auto-sets Content-Type) |
| `.body(bytes)` | Set raw body |
| `.form(fields)` | Set URL-encoded form body |
| `.file(field, filename, content_type, data)` | Add a multipart file part |
| `.field(name, value)` | Add a multipart text field |
| `.multipart()` | Finalize the collected file/field parts into a `multipart/form-data` body |
| `.cookie(name, value)` | Add a cookie |
| `.query(key, value)` | Add a query parameter |
| `.queries(pairs)` | Add multiple query parameters |
| `.send().await` | Execute the request |

### TestResponse Methods

| Method | Checks |
|--------|--------|
| `assert_ok()` | Status 200 |
| `assert_created()` | Status 201 |
| `assert_no_content()` | Status 204 |
| `assert_bad_request()` | Status 400 |
| `assert_unauthorized()` | Status 401 |
| `assert_forbidden()` | Status 403 |
| `assert_not_found()` | Status 404 |
| `assert_conflict()` | Status 409 |
| `assert_unprocessable()` | Status 422 |
| `assert_too_many_requests()` | Status 429 |
| `assert_internal_server_error()` | Status 500 |
| `assert_status(code)` | Arbitrary status |
| `assert_json_path(path, expected)` | JSON path equals value |
| `assert_json_path_fn(path, predicate)` | JSON path satisfies predicate |
| `assert_json_contains(expected)` | JSON subset match |
| `assert_json_path_contains(path, item)` | JSON path subset match |
| `assert_json_shape(schema)` | Type structure match |
| `assert_header(name, expected)` | Header equals value |
| `assert_header_exists(name)` | Header exists |
| `json::<T>()` | Deserialize body into `T` |
| `json_path::<T>(path)` | Deserialize value at path |
| `text()` | Body as `String` |
| `header(name)` | Get header value |
| `cookie(name)` | Get cookie from Set-Cookie |
| `cookies()` | Get all Set-Cookie values |

All `assert_*` methods return `&Self` for chaining:

```rust
app.get("/users")
    .bearer(&token)
    .send()
    .await
    .assert_ok()
    .assert_json_path("len()", 3)
    .assert_json_shape(serde_json::json!([{"id": 0, "name": ""}]));
```

## TestJwt API

| Method | Description |
|--------|-------------|
| `TestJwt::new()` | Create with default secret/issuer/audience |
| `TestJwt::with_config(secret, issuer, audience)` | Create with custom config |
| `token(sub, roles)` | Generate a JWT with subject and roles |
| `token_with_claims(sub, roles, email)` | Generate a JWT with optional email |
| `token_builder(sub)` | Start a `TokenBuilder` for custom claims |
| `validator()` | Return a `JwtValidator` for these tokens |
| `claims_validator()` | Return a `JwtClaimsValidator` for these tokens |

### TokenBuilder Methods

| Method | Description |
|--------|-------------|
| `.roles(roles)` | Set roles |
| `.email(email)` | Set email claim |
| `.claim(key, value)` | Add a custom claim |
| `.expires_in_secs(secs)` | Set expiration (default: 3600) |
| `.expired()` | Set `exp` to 60 seconds in the past |
| `.build()` | Sign and return the JWT string |

### Generated Tokens

Tokens are signed with HMAC-SHA256 and contain:

```json
{
    "sub": "user-1",
    "roles": ["user"],
    "iss": "r2e-test",
    "aud": "r2e-test-app",
    "exp": 1706130000
}
```

## Additional Helpers

Beyond the in-process client, `r2e-test` re-exports a few specialized helpers:

- **`WsTestClient`** (feature `ws`) — a real WebSocket client for `#[ws]` endpoints. Boot a live server with `TestApp::serve().await` (returns a `TestServer`), then `server.ws(path)` connects; the client exposes `send_text/send_json/send_binary`, `next_text/next_json/next_binary`, `close`, and `assert_no_message`.
- **`FiniteStream` / `ParsedSseEvent`** — consume and parse SSE responses; `TestResponse` also has `sse_events`, `assert_sse_event`, and `assert_sse_data`.
- **`TestServer`** — a live TCP server (via `TestApp::serve()`) for cases that need a real socket rather than `oneshot` dispatch. On a booted `TestApp` it runs on the app's tracked lane, so `app.shutdown()` drains it under `drain_timeout`; dropping the `TestServer` first stops it on its own.
- **`SetCookie`** — parsed `Set-Cookie` attributes, with `TestResponse` helpers `assert_cookie_secure`, `assert_cookie_http_only`, `assert_cookie_same_site`, `assert_cookie_path`.

## Dev Services (real infrastructure)

When a test needs a real database or broker rather than a mock, `r2e-devservices`
(features `postgres`, `redis`, `openfga`) starts it in Docker and hands back the
URL to inject into the test config:

```rust
use r2e_devservices::DevPostgres;

#[r2e::test]
async fn users_are_persisted() {
    let pg = DevPostgres::shared().await;
    let app = TestApp::boot_with::<my_app::MyApp>(|b| {
        b.override_config_value("app.database.url", pg.url())
    })
    .await;
    // ...
}
```

`shared()` reuses one container across every test process of the workspace
session (a Ryuk reaper removes it after the last one exits); `start()` gives an
isolated container tied to the returned handle. Tests on a shared container must
not assume exclusive state — namespace per test (a dedicated schema, a unique
store name) or take `start()`.

### Choosing the image and credentials

`shared_with` / `start_with` take a spec. Everything in it is part of the
container's identity, so each distinct spec gets its own shared container:

```rust
use r2e_devservices::{DevPostgres, DevRedis, PostgresImage, PostgresSpec, RedisImage};

// A distribution shipping extra extensions.
let pg = DevPostgres::shared_with(PostgresImage::new("pgvector/pgvector", "pg18")).await;

// Custom credentials — the URL follows them.
let app_db = DevPostgres::shared_with(
    PostgresSpec::default()
        .with_user("app")
        .with_password("s3cret")
        .with_database("appdb"),
)
.await;

let valkey = DevRedis::shared_with(RedisImage::new("valkey/valkey", "8-alpine")).await;
```

Defaults are `postgres:16-alpine` (`postgres`/`postgres`, database `postgres`)
and `redis:7-alpine`. A Postgres image must speak Postgres on 5432 and honour
`POSTGRES_USER` / `POSTGRES_PASSWORD` / `POSTGRES_DB`.

### Any other service

`DevService` is the generic form the wrappers above are built on, available
without any feature flag. Any testcontainers `Image` — a `testcontainers-modules`
one, a `GenericImage`, or your own — gets the same labelling, reaping and
cross-process sharing:

```rust
use r2e_devservices::testcontainers::core::{IntoContainerPort, WaitFor};
use r2e_devservices::testcontainers::{GenericImage, ImageExt};
use r2e_devservices::{DevService, DevServiceSpec};

let clickhouse = DevService::shared(
    DevServiceSpec::new("clickhouse", || {
        GenericImage::new("clickhouse/clickhouse-server", "24.8-alpine")
            .with_exposed_port(8123.tcp())
            .with_wait_for(WaitFor::message_on_either_std("Ready for connections"))
            .into()
    })
    .with_port(8123),
)
.await;
let url = format!("http://{}", clickhouse.endpoint(8123));
```

`testcontainers` and `testcontainers_modules` are re-exported so the spec builds
against the versions this crate uses (a mismatched one yields a different
`ContainerRequest` type and will not compile). A ready-made module image
(`ClickHouse`, `Kafka`, …) needs its own feature enabled through your
`[dev-dependencies]`: `testcontainers-modules = { version = "0.15", features =
["clickhouse"] }`. The closure must be deterministic — it is called again for
the identity and on each start attempt, and the shared path panics rather than
start a container its name does not describe. It builds the request on demand
because `ContainerRequest` is not `Clone` and a contended start is retried.
`with_port` resolves a port the image exposes; it does not publish one.

Two specs share a container when their identity matches, and that identity is
derived from the request — the fields that shape the container Docker creates:
image, env vars, labels, command, mounts, port mappings, device requests,
network, … — each folded the way Docker resolves it (keyed fields keep the
effective value, set-like fields are sorted, ordered fields stay in order), so
anything that changes the container separates it. Only what stays outside the
request needs help: ulimits (testcontainers keeps them private), the contents of
a file copied by path, and anything applied after start — seeded data, or exec
hooks the image runs itself. A host-config modifier is not merely invisible, it
is refused: `shared` panics on a spec that sets one without a discriminator,
since its effect is a closure and guessing would merge two different containers.

```rust
DevServiceSpec::new("clickhouse", request)
    .with_port(8123)
    .with_discriminator("seeded-fixtures-v2")
```

`R2E_DEVSERVICES_KEEP=1` disables reaping for post-mortem inspection; the
remaining knobs are documented in `r2e-devservices/README.md`.

## Running Tests

```bash
# All tests in the workspace
cargo test --workspace

# Tests for a specific crate
cargo test -p example-app

# A specific test
cargo test -p example-app test_health_endpoint
```

## Validation Criteria

```bash
cargo test --workspace
# All tests pass (integration + unit)
```
