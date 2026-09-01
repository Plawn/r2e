---
topic: testing
features: core (dev-dependency `r2e-test`)
tokens: ~4300
requires: quick-start, app-builder, devservices
---

## Testing

### TL;DR

- Boot the real app: `#[r2e::test(app = my_app::MyApp)]` or
  `TestApp::boot::<my_app::MyApp>().await` — never re-declare controllers or a
  router in a test; `app = ...` is the app TYPE.
- Pin mocks and patch config in the `with = |b| b.override_bean(...)
  .override_config_value(...)` hook — it runs before `App::build`.
- Boot forces the `test` profile and wires `TestJwt`, so
  `.as_user("alice", &["user"])` mints valid Bearer tokens; use `.as_tenant(...)`
  / `.as_tenant_user(...)` for multi-tenant apps.
- A failed boot is a failing test (panic with the `caused by:` chain); use
  `try_boot*` when the boot is *expected* to fail.
- Sequence tests with `order = N` (+ optional `group = "..."`) inside one test
  binary; `group` without `order` is a compile error.
- Use `#[r2e::test_suite]` when cases share one instance and one runtime;
  `#[ignore]` on a `#[case]` is a compile error — skip in the case body.
- Share `App::Env` across boots with a `static SharedEnv<A>` + `env = ENV.get().await`;
  never memoise it in a bare `OnceCell` / `LazyLock`.
- Call `app.shutdown().await` in tests that assert on disposal — dropping a
  `TestApp` runs no hooks and aborts tracked tasks.
- A test boot skips the plugin serve hooks, so `#[scheduled]` tasks do not tick;
  use `app.serve()` for a live port (WebSocket/SSE).
- Container-backed dependencies (Postgres, Redis, OpenFGA, Keycloak): see
  llm/devservices.md.

The `App` trait makes tests boot the **real app** — never re-declare
controllers/routers in tests. `app = ...` is the app **TYPE** (implements `App`).

```rust
use r2e::prelude::*;
use r2e_test::{TestApp, TestJwt};

// Macro form — binds params: app: TestApp, jwt: TestJwt, #[inject] bean: T
#[r2e::test(app = my_app::MyApp)]
async fn lists_users(app: TestApp) {
    app.get("/users").as_user("alice", &["user"]).send().await
        .assert_ok()
        .assert_json_path("/0/name", "Alice");
}

// With overrides: pin mocks and patch config BEFORE `build` runs
#[r2e::test(app = my_app::MyApp, with = |b| b
    .override_bean(MockMailer::new())
    .override_config_value("app.llm.api_key", "test-key"))]
async fn with_mocks(app: TestApp, #[inject] service: UserService) { /* … */ }

// Ordered tests (@Order): `order = <u32>` runs tagged tests sequentially in
// ascending order within the SAME test binary (one file under tests/).
#[r2e::test(app = my_app::MyApp, order = 1)]
async fn creates_user(app: TestApp) { /* … */ }

#[r2e::test(app = my_app::MyApp, order = 2)]
async fn lists_created_user(app: TestApp) { /* … */ }

// Optional `group = "..."`: independent sequences in one binary; a test waits
// only on lower orders of its OWN group (default = the unnamed group).
#[r2e::test(app = my_app::MyApp, order = 10, group = "billing")]
async fn billing_step(app: TestApp) { /* … */ }

// Suite form: one Cargo test per #[case], one shared suite instance.
struct UserSuite {
    app: TestApp,
    service: UserService,
    user_token: String,
}

#[r2e::test_suite(app = my_app::MyApp, with = |b| b.override_bean(MockUsers::new()))]
impl UserSuite {
    #[before_all]
    async fn setup(app: TestApp, #[inject] service: UserService, jwt: TestJwt) -> Self {
        let user_token = jwt.token("alice", &["user"]);
        Self { app, service, user_token }
    }

    #[before_each]
    async fn reset(&mut self) { self.service.clear().await; }

    #[case]
    async fn creates_user(&mut self) {
        self.app.post("/users").bearer(&self.user_token).send().await.assert_ok();
    }

    #[case(order = 10)]
    async fn lists_user(&mut self) { /* ordered relative to ordered suite cases */ }

    #[after_each]
    async fn cleanup(&mut self) { /* … */ }

    #[after_all]
    async fn teardown(&mut self) { /* full-suite runs only */ }
}
# fn main() {}
```

Ordered-test rules (copy literally):
- Scope is the test binary — no cross-binary/cross-crate ordering. Orders need
  not be contiguous (10, 20, 30 is fine). Works with or without `app` (with
  `app`, the barrier covers the TestApp boot too).
- Tests WITHOUT `order` are unaffected and stay parallel — no `--test-threads=1`.
- Fail-fast: a FAILING ordered test — panic, or `Err` from a `Result` test —
  poisons its group; later tests of that group fail immediately naming the
  failed predecessor (no deadlock). A `#[should_panic]` ordered test that
  panics as expected is a pass and does NOT poison its group.
- Duplicate `order` in one group → runtime panic naming both tests.
- Watchdog: a waiting test panics (not hangs) if some lower order was NEVER
  STARTED and the group stays idle for `R2E_TEST_ORDER_TIMEOUT_SECS` (default
  60) — e.g. a lower order was filtered out by `cargo test <filter>` or starved
  by `--test-threads`. A running (started) predecessor never trips the watchdog,
  however slow.
- `group` without `order` is a compile error; `order`/`group` on `#[r2e::main]`
  are compile errors.

Suite-test rules:
- `#[r2e::test_suite]` goes on a non-generic inherent `impl`; helper attributes
  are `#[case]`, `#[before_all]`, `#[before_each]`, `#[after_each]`,
  `#[after_all]` (camelCase aliases accepted).
- `#[before_all]` is optional. If it returns `Self` / `Result<Self, E>`, that
  constructs the suite; otherwise the suite type must implement `Default`.
- Cases are unordered by default. `#[case(order = N)]` opts into the existing
  ordered-test barrier within that suite.
- `after_all` runs when the last generated case completes, counted against the
  `#[case]`s the macro emitted: a partial `cargo test <filter>` run never
  reaches it, because libtest does not expose the selected case set.
- `#[ignore]` on a `#[case]` is a **compile error** — it would either suppress
  teardown or let teardown run before the ignored case. Skip in the case body.
- **One runtime per suite**, not per case: `#[before_all]`, `#[before_each]`,
  every `#[case]`, `#[after_each]` and `#[after_all]` run on the same reactor,
  owned by the suite. So a `#[before_all]` may amortise runtime-bound
  resources — a `TestApp`, a database pool, a socket, a spawned task, a timer —
  and every case (and `after_all`) still finds them alive. The runtime knobs
  (`flavor`, `worker_threads`, `start_paused`, …) are declared on
  `#[r2e::test_suite(...)]` and apply to that one runtime; a `start_paused`
  suite therefore shares one paused clock across its cases. Knob combinations
  the runtime builder panics on (`start_paused` without
  `flavor = "current_thread"`, a zero `worker_threads`, …) are compile errors.
  A phase that somehow runs off the suite runtime panics naming both runtimes
  instead of timing out on a dead resource.
- **Teardown**: after `#[after_all]`, the last case drops the suite value inside
  the suite runtime and then shuts that runtime down, so suite threads and
  detached tasks do not outlive the suite. Use after teardown panics by name.
- Using `order` requires the `r2e-test` dev-dependency
  (already present when using `app = …`).

Explicit form (turbofish TYPE args): `TestApp::boot::<my_app::MyApp>().await`,
`TestApp::boot_with::<my_app::MyApp>(|b| ...).await`,
`TestApp::boot_plain::<my_app::MyApp>(|b| ...).await`. Boot forces the `test`
profile (`application-test.yaml` overlays) and wires a `TestJwt` validator so
`.as_user(sub, &roles)` mints valid Bearer tokens. For multi-tenant apps:
`.as_tenant("acme")` (sets the `x-tenant-id` header; also on `TestSession`
requests) and `.as_tenant_user("alice", "acme", &["admin"])` (Bearer token with
a `tenant` claim **and** the header, so header- and claim-based resolvers both
work). The constants are `r2e_test::{TENANT_HEADER, TENANT_CLAIM}`.

A boot failure is a **failing test**, never a dead runner: `boot`/`boot_with`/
`boot_plain` panic with `TestApp::boot::<MyApp>() failed: <error>` plus the
`caused by:` chain, which libtest attributes to the test that called it. Use
`TestApp::try_boot::<A>()` / `try_boot_with` / `try_boot_plain` (all returning
`Result<TestApp, BootError>`) to assert on a boot that is *expected* to fail.
Both forms cover the whole startup, not just `setup`/`build`: config loading,
bean and producer construction, plugin build, and the controller
`#[post_construct]` / `#[on_start]` hooks the harness runs (`TestApp` boots
through `try_build_with_consumers`).

#### Sharing one `App::Env` across boots

`App::Env` is the production concept of "resources built once". Every plain
`boot*` calls `A::setup()` again, so a test binary that boots per test replays
the pools and migrations per test. The `*_env` boots take an environment the
caller already owns and hand it straight to `A::build` — `A::setup()` is not
called at all:

```rust
use r2e_test::{SharedEnv, TestApp};

// `const`, so it goes straight into a `static`. `setup()` runs ONCE per test
// binary, on a runtime `r2e-test` owns and never shuts down.
static ENV: SharedEnv<MyApp> = SharedEnv::new();

#[r2e::test(app = my_app::MyApp, env = ENV.get().await)]     // macro knob
async fn lists_users(app: TestApp) {
    app.get("/users").send().await.assert_ok();
}

// Explicit forms, mirroring boot / boot_with / boot_plain:
# async fn __doc() {
let app = TestApp::boot_env::<MyApp>(ENV.get().await).await;
let app = TestApp::boot_with_env::<MyApp>(ENV.get().await, |b| b.override_bean(mock)).await;
let app = TestApp::boot_plain_env::<MyApp>(ENV.get().await, |b| b).await;
// `try_boot_env` / `try_boot_with_env` / `try_boot_plain_env` are the
// `Result`-returning forms.
# }

// "setup, then seed once":
static SEEDED: SharedEnv<MyApp> = SharedEnv::with(|| Box::pin(async {
    let env = MyApp::setup().await?;
    seed_reference_data(&env).await?;
    Ok(env)
}));
```

`SharedEnv<A>` API: `new()` / `with(init)` (both `const`), `get().await ->
A::Env` (panics with the app name + full `caused by:` chain when `setup`
failed), `try_get().await -> Result<A::Env, SharedEnvError>` (`app()`,
`chain()`), `shared_env_runtime() -> RuntimeHandle` for a fixture that must
spawn onto the same long-lived runtime. Concurrent first callers share one
`setup` run; a failed environment stays failed for the process (no retry — the
failed attempt's side effects already happened).

**Never memoise the environment in a bare `OnceCell`/`LazyLock`.** `#[r2e::test]`
builds one runtime per test and drops it at the end of the test;
`#[r2e::test_suite]` builds one per suite and shuts it down after the last case.
A `OnceCell` initialised from a test runs `setup()` on whichever per-test runtime
won the race, so everything the environment bound to it (listeners, pool
keep-alive tasks, timers, anything `setup` spawned) dies with that runtime while
the value lives on in the `static` — later tests then hang on an inert
environment. A `LazyLock` that builds its own runtime and `block_on`s it panics
outright inside a test runtime. `SharedEnv` exists precisely to fix that
lifetime.

`env = <expr>` is also accepted on `#[r2e::test_suite(app = …)]`, so several
suites in one binary share the same environment (`#[before_all]` only amortises
within its own suite). It requires a `#[before_all]` that binds the booted app
(`async fn setup(app: TestApp) -> Self`) — that hook is what evaluates the
expression; without it the expression would never run, so it is a compile error
(same for `with = …` / `jwt = …`). Everything else is unchanged: `test` profile,
pinned `TestJwt` validators, the production startup phase, `shutdown()`.

**Isolation is the caller's job.** A shared `Env` means shared state — the same
pool, rows and caches for every test that boots off it, with the boots running
concurrently under libtest. Keep tests independent (per-test schema, prefix or
tenant), or serialise them with `order = …`.

**Shutdown runs whatever `A::build` registered.** The harness never disposes the
`Env` itself, but it cannot promise nothing else does: `shutdown()` runs the
app's own shutdown sequence (`on_drain`, disposers, `#[pre_destroy]`, `on_stop`)
over what `build` registered. If `build` hands an `Env`-owned resource to
something that closes it, the first `shutdown()` invalidates it for every later
boot — the app's contract, not the harness's.

### Test lifecycle: startup and shutdown

`TestApp::boot` runs the **production startup phase**, not just a router build:
controller `#[post_construct]`, consumer registrations, bean/controller
`#[on_start]`, then the builder's `.on_start(…)` closures — which is what starts
`spawn_service` / `#[derive(BackgroundService)]` tasks. An `Err` from any of them
is the boot failure above.

`app.shutdown().await` runs the matching **production shutdown sequence**, under
the app's own budgets:

```rust
#[r2e::test(app = my_app::MyApp)]
async fn disposes_cleanly(app: TestApp) {
    app.post("/orders").send().await.assert_ok();
    app.shutdown().await;   // on_drain → disposers → cancel → join → on_stop
}
```

The phases and their budgets are the production ones — see llm/app-builder.md;
the server behind `app.serve()` is one of the tracked handles joined there.

`shutdown()` is the **OS-signal path**: it does not call `StopHandle::stop()`,
because production does not either (`run()` waits on
`select!(shutdown_signal(), stop_handle.stopped())`, so under SIGTERM the handle
never fires and `is_stopped()` reads `false` throughout). To exercise the
programmatic stop instead — what an admin `/shutdown` endpoint triggers — fire
it yourself first: `app.stop_handle().stop();` then `app.shutdown().await`.
Readiness only flips if the app's own `on_drain` hook flips it; no R2E code
does that for you.

`shutdown()` is explicit because `Drop` cannot await. Dropping a `TestApp`
cancels the app token and then **aborts** every still-running tracked task
(background services, an attached `app.serve()` server) — nothing would ever
join those handles, so dropping them alone would detach the tasks, not stop
them. The hooks do not run; a drop with work pending logs a warning (it does
not name the app — `RunningApp` is type-erased). Call `shutdown()` in tests
that assert on disposal, or that need a cooperative service's cleanup to
finish. `app.has_shutdown_work()` is `false` only when dropping loses nothing:
no hook registered, no unfired plugin hook, no live tracked task.

What a test boot skips: the **plugin serve hooks** — they bind ports
(separate-port gRPC, MCP) and start the scheduler driver — so `#[scheduled]`
tasks do not tick under `TestApp` and WebSocket sessions stay untracked
(above); plus the two behaviours that *are* the listener, SO_REUSEPORT sharded
serving and QUIC/HTTP3. An in-process start spawns no worker runtimes, so a
registered `per_worker_service()` is a boot error rather than a silently dead
service; an invalid `server.workers` is a boot error there exactly as in
production.

For a fully in-memory config in a test, stash one with
`.override_config(R2eConfig::from_yaml_str(yaml)?)` in the `with` hook — the
next `load_config::<C>()` uses it instead of reading disk (see Configuration).

CLI test runner:
- `r2e test` is the blessed wrapper for project tests; default behavior is
  equivalent to `cargo test`.
- `r2e test --coverage` runs `cargo llvm-cov` and prints the coverage summary.
- `r2e test --sonarqube` implies coverage and writes
  `coverage/lcov.info` via `cargo llvm-cov --lcov --output-path`; configure
  SonarQube with `sonar.rust.lcov.reportPaths=coverage/lcov.info`.
- `r2e test --sonarqube --output-path reports/lcov.info` changes the LCOV path.
- Cargo scoping/options: `--workspace`, repeatable `-p/--package`,
  `--features`, `--all-features`, `--no-default-features`; args after `--`
  go to the test binary (e.g. `r2e test -- --nocapture`).
- `r2e test` does **not** run `sonar-scanner`; CI runs scanner after the LCOV
  report exists. Coverage prerequisites: `cargo install cargo-llvm-cov` and
  `rustup component add llvm-tools-preview`.

- `app.bean::<T>()` — fetch any bean from the booted graph; `app.config()`.
- `TestRequest`: `.bearer(t)`, `.header(...)`, `.json(body)`, `.form(...)`,
  `.query(k, v)`, `.file(...)` — then `.send().await`.
- `TestResponse` assertions: `assert_ok/created/no_content/bad_request/unauthorized/
  forbidden/not_found/...`, `assert_json_path(path, expected)`,
  `assert_json_contains(subset)`, `assert_json_shape(schema)`, `assert_header(...)`,
  cookie and SSE assertions; access via `json::<T>()`, `text()`.
- `app.session()` — cookie-persisting `TestSession`.
- `app.serve()` — live `TestServer` on a random port (WebSocket/SSE tests);
  `WsTestClient` for WS. On a booted app the server runs on the app's tracked
  lane, so `app.shutdown()` drains it under `drain_timeout`; dropping the
  `TestServer` first stops that server on its own, under the same
  `drain_timeout` (the budget starts from whichever trigger fired).
- `app.shutdown().await` — production shutdown sequence (see above);
  `app.has_shutdown_work()` reports whether shutdown has anything to do — user
  hooks, unfired plugin sync hooks, or a live tracked task.
- `app.stop_handle()` — the app's `StopHandle`; fire it before `shutdown()` to
  take the programmatic-stop path instead of the signal path.
- `TestJwt::token_builder(sub)` — expired/wrong-issuer/wrong-audience tokens.
