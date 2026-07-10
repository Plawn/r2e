# Plan — Testing DX ("as practical as Quarkus")

Status: **Phase 1 done, Phase 2 (Postgres/Redis) done** (2026-07-09).
Decided with the user on 2026-07-09: Phases 1 + 2 approved; blueprint
approach = macro-driven (`#[r2e::test(app = ...)]`).

## Motivation

The HTTP client side of `r2e-test` (assertions, sessions, WS/SSE, `TestJwt`)
is already strong. The gaps versus Quarkus are all on the *bootstrap* side:

1. No `@QuarkusTest`: every test file hand-writes a 15–45 line `setup()`, and
   because apps are binaries, controllers/services get **copy-pasted into
   tests** (see the old `examples/example-app/tests/user_controller_test.rs`).
2. No `@InjectMock`: `allow_bean_override` + `override_provide` existed but
   were last-wins and unusable with a blueprint pattern (see below); `TestApp`
   held only the `Router`, so tests could not reach beans.
3. No `@TestProfile`: config was built key-by-key; no `test` profile, no
   `application-test.yaml`, no per-test key patching.
4. No `@TestSecurity` inline: `jwt.token(...)` + `.bearer(...)` on every call.
5. No Dev Services (phase 2) and no continuous testing (phase 3, not planned).

## Core design decision: pinned overrides

Target usage: the app exposes a **blueprint** function; the test harness
prepares the builder, hands it to the blueprint, and gets back the built app.

```rust
// app lib.rs
pub async fn app(b: AppBuilder) -> impl BootableApp {
    b.load_config::<AppConfig>()
        .provide(...)
        .register::<UserService>()
        .build_state().await
        .with(Health)
        .register_controllers::<(UserController, ...)>()
}
```

Because the harness runs **before** the app's own `provide()`/`register()`
calls, last-wins override semantics are backwards. Instead, test overrides are
**pinned**: `override_bean(mock)` wins over any *later* registration of the
same `TypeId`, which is silently ignored. This gives `@InjectMock` semantics
without touching the compile-time provision list `P` (the app's own
registration provides the P slot; the pin only substitutes the value).

The same pre-configuration idea covers config and profile:

- `override_config_value(key, value)` — stashed on the builder, applied on top
  of whatever `with_config`/`load_config` loads (`@TestProfile`
  `getConfigOverrides()` equivalent).
- `with_profile(name)` — forces the active profile (priority above
  `R2E_PROFILE` env; no process-global env mutation in parallel tests).
- `R2eConfig` loading overlays `application-{profile}.yaml` on top of
  `application.yaml` when the file exists (useful in prod too:
  `application-prod.yaml`).

## Phase 1 — the harness

### r2e-core

- `BeanRegistry`: `pinned: HashSet<TypeId>` + `pin_provide<T>(value)`.
  `provide`, `register_inner`, `register_async_inner`,
  `register_producer_inner`, `provide_factory_with_config` no-op when the
  TypeId is pinned.
- `AppBuilder<NoState>`: `override_bean(value)` (Self→Self — usable inside
  `boot_with` tweak closures), `override_config_value(key, value)`,
  `with_profile(name)`.
- `with_config` / `load_config` apply stashed config overrides after loading
  and resolve the profile as: forced > `R2E_PROFILE` > `r2e.profile` >
  `default`. `load_config` overlays `application-{profile}.yaml`.
- **Removed** (breaking, replaced by pinning): `allow_bean_override`,
  `override_provide`, and the registry's `allow_overrides` last-wins dedup
  for this path (the default/alternative `overridable` flag machinery is
  unrelated and stays).
- New trait `BootableApp` (blueprint return contract), implemented by the
  typed `AppBuilder<T>`:
  `bean_context()`, `r2e_config()`, `into_router()`, `serve_auto()`.
  Blueprints return `impl BootableApp`; production `main` still calls
  `serve_auto()` through it.

### r2e-test

- `TestApp` gains `bean_context`, `config`, `jwt` fields.
  - `TestApp::boot(app_fn).await` — prepares
    `AppBuilder::new().with_profile("test")`, pins
    `Arc<JwtClaimsValidator>` + `Arc<JwtValidator>` from a fresh `TestJwt`,
    calls the blueprint, extracts router + bean context + config.
  - `TestApp::boot_with(app_fn, |b| ...).await` — same, with a tweak hook for
    `override_bean` / `override_config_value`.
  - `boot_plain` variants skip the TestJwt pinning (apps with custom
    validators/role extractors).
- `app.bean::<T>()` — fetch any bean from the resolved graph (the
  `@Inject`-into-the-test-class equivalent).
- `app.test_jwt()` — the auto-wired `TestJwt`.
- `.as_user(sub, &roles)` on `TestRequest` and `SessionRequest` — mints a
  token from the app's `TestJwt` and sets the Bearer header
  (`@TestSecurity` equivalent).

### r2e-macros

`#[r2e::test]` grows blueprint support:

```rust
#[r2e::test(app = example_app::app)]
async fn lists_users(app: TestApp) {
    app.get("/users").as_user("u1", &["user"]).send().await.assert_ok();
}

#[r2e::test(app = example_app::app, with = |b| b.override_bean(FakeMailer::new()))]
async fn with_mock(app: TestApp, #[inject] mailer: FakeMailer) { ... }
```

- New args: `app = <path>` (blueprint fn), `with = <closure>` (builder tweak),
  `jwt = false` (skip TestJwt auto-wiring).
- Parameter binding: type `TestApp` → the booted app; type `TestJwt` → the
  app's jwt; `#[inject] x: T` → `app.bean::<T>()`. Anything else is a compile
  error with a hint.
- `crate_path::r2e_test_path()` resolves `r2e-test` (fallback `::r2e_test`).

### example-app + cleanup

- `examples/example-app` becomes lib + bin; `src/lib.rs` exposes
  `pub async fn app(b: AppBuilder) -> impl BootableApp`. Tests import the real
  controllers instead of copy-pasting them.
- Showcase: `examples/example-app/tests/app_test.rs` boots the real app and
  demos every Phase-1 feature (as_user, roles, #[inject] params, pinned mock,
  config patch, `application-test.yaml`). The pre-existing test files keep
  their inline-controller style — they test framework features, not the app.
- `r2e generate crud` test template now emits blueprint-style tests
  (`#[r2e::test(app = <lib>::app)]`).
- Remove `#[derive(TestState)]` (pre-HList vestige, unused).
- Update `docs/features/12-testing.md`, `docs/claude/subsystems.md`,
  root `CLAUDE.md` crate table.

## Phase 2 — Dev Services (testcontainers)

Done for Postgres + Redis: crate `r2e-devservices` (features `postgres`,
`redis`) on testcontainers 0.27 / testcontainers-modules 0.15.

Design: **explicit and composable**, not implicit config-sniffing —

```rust
let pg = DevPostgres::shared().await;   // one container per test process
let app = TestApp::boot_with(my_app::app, |b| {
    b.override_config_value("app.database.url", pg.url())
}).await;
```

- `shared()` starts the container once per process (tokio `OnceCell`) and
  keeps it until process exit (testcontainers' reaper then removes it);
  `start()` / `start_with_tag(tag)` give isolated containers.
- Image tags are pinned (`postgres:16-alpine`, `redis:7-alpine`) — the
  modules' own defaults (`postgres:11-alpine`, `redis:5.0`) predate arm64
  and die with `exec format error` on Apple Silicon.
- Docker-backed smoke tests are `#[ignore]`d; run with
  `cargo test -p r2e-devservices --features postgres,redis --test dev_services -- --ignored`
  (scoped to the test file because the repo's `ignore` doctests would be
  swept up by a bare `-- --ignored`).

Follow-ups (not started): Kafka / RabbitMQ / Pulsar / OpenFGA services; a
real-app demo in `examples/example-postgres`; optional auto-start keyed off
missing config (deliberately deferred — implicitness hides failures).

## Phase 3 — continuous testing (`r2e test --watch`)

Explicitly deferred; not approved yet.

## Non-goals / notes

- Mocking stays type-identity-based: an override must be the same Rust type
  as the bean it replaces (trait-object beans or test-configured instances).
  There is no proxy/subclass magic like Mockito — document the pattern.
- Boot is per-test (in-process, no TCP) — cheap enough that Quarkus-style
  "boot once per profile" caching is not needed initially.
