# Changelog — r2e-core

All notable changes to `r2e-core` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the version is this
crate's own (pre-1.0: a breaking change bumps the minor, anything else the
patch).

Write entries under `[Unreleased]` in the PR that makes the change. A release
PR (`scripts/bump-version.sh`) turns that section into `[X.Y.Z] - date`, and
`.github/workflows/release.yml` publishes the crate, tags `r2e-core-vX.Y.Z` and
uses the section as the GitHub release notes.

Up to 0.6.0 the whole workspace was released in lockstep; that history lives
in the root [`CHANGELOG.md`](../CHANGELOG.md).

## [Unreleased]

## [0.9.0] - 2026-10-31

### Breaking

Migration guide: [`docs/migration/tracing-single-path.md`](../docs/migration/tracing-single-path.md).

- **One tracing path.** `build_state()` installs the process-global
  subscriber, first thing, from the loaded `tracing:` section plus the layers
  contributed with `.tracing_layer(..)`; nothing else installs it. Removed
  (each leftover use fails to compile naming the guide): the `Tracing` and
  `ConfiguredTracing` plugins (`ConfiguredTracing` also leaves the prelude),
  `init_tracing`, `init_tracing_from_config`, `init_tracing_with_config`,
  `try_init_tracing_with_config`, `ensure_tracing_installed`;
  `LaunchOptions.tracing` (`LaunchOptions` is now field-less and
  `#[non_exhaustive]`); `SubscriberAlreadyInstalled` (use `AlreadyInstalled`),
  `resolve_tracing_options`, `tracing_options_of` and the `TracingOptions` →
  `TracingConfig` conversion. `#[r2e::main]`, `#[r2e::test]` and
  `#[r2e::test_suite]` no longer install a subscriber (`tracing = …` is
  rejected by `r2e-macros`).
- The process-global subscriber registries were removed with `r2e-tracing`'s
  surface: `contribute_layer`, `late_layers`, `LateLayers`, `NotWired`,
  `suppress`, `suppress_error`, `is_suppressed`, `NoiseRule`, `NoiseFilter`,
  `add_enricher`, `FieldEnricher`, `EnrichContext`, `set_console_factory`,
  `ConsoleFactory`. Contribute a layer instead: `Suppress::error(target,
  message)` for a benign event, a layer writing `LogFields` for console
  fields, `tracing.console: false` + your own `fmt` layer for a custom
  console.
- `TracingConfig` is `#[non_exhaustive]` (build it with `default()` + `with_*`
  or bind it), gains `enabled` and `console`, and `JsonLogConfig` is its
  nested `json` field (`tracing.json.*` keys unchanged).

### Changed

- The tracing subscriber primitives live in the new `r2e-tracing` crate
  (re-exported as `r2e_core::r2e_tracing`); `r2e-core` no longer depends on
  `tracing-subscriber` directly.
- An invalid tracing filter (`tracing.filter`) no longer panics the install:
  the subscriber is installed with `info` and the parse error is logged at
  ERROR through it. An invalid `tracing:` section installs the built-in
  defaults and logs one ERROR.
- With `ansi` unset, console colours are on only when stdout is a terminal
  (piped or collected logs no longer carry escape codes).
- A subscriber installed before `build_state()` by someone else wins as
  before, but contributed layers are no longer silently lost: one WARN says
  they were ignored (`tracing.enabled: false` silences it).

### Added

- `AppBuilder::tracing_layer(layer)` and `PluginSetupContext::tracing_layer(layer)`:
  contribute any `Layer<Registry> + Send + Sync + 'static` to the subscriber
  `build_state()` installs.
- `tracing.enabled` (default `true`; `false` installs nothing and drops the
  boot-log buffer — the app owns the subscriber) and `tracing.console`
  (default `true`; `false` keeps only the contributed layers).
- `tracing.json.*` keys (`JsonLogConfig`): `flatten`, `message-key`,
  `current-span`, `span-list` — with `tracing.format: json` and
  `flatten: true` the console writes flat JSON lines (VictoriaLogs:
  `message-key: _msg`).
- Boot-log buffer: `launch` / `launch_with` call `capture_boot_logs()` before
  `App::setup`, so INFO+ events logged before the install (setup, config
  loading, plugin setup) are replayed through the real subscriber; a boot
  that fails before the install writes them to stderr ahead of the error
  report.
- Re-exported at the crate root: `Suppress`, `LogFields`, `BoxedLayer`,
  `TracingOptions`, `JsonOptions`, `JsonLogConfig`, `capture_boot_logs`,
  `BootLogs`, `installed`, `AlreadyInstalled`.
- `SerdeConfig<T>`: load any `T: serde::Deserialize + Clone` as a typed config
  section without `#[derive(ConfigProperties)]` — root (`load_config`),
  `provide_config`, plugin `type Config` or `#[config(section)]` field. `T` is
  registered as a bean next to the wrapper. Serde errors name the r2e key of
  the offending value (`app.engines.1.weight`). In exchange for full serde
  shapes it has no aggregated missing-key report and no `PropertyMeta` (#1180).
- `R2eConfig::tree(prefix)`: rebuild a nested `ConfigValue` subtree from the
  flat keys, so `R2E_*` and `override_config_value` overrides of indexed keys
  are visible (#1180).
- `deserialize_value` (and so `#[derive(FromConfigValue)]`) now deserializes
  straight from the `ConfigValue` instead of going through `serde_json`, and
  coerces strings on typed hints with the `FromConfigValue` rules
  (`"8080"` → `u16`, `"yes"` → `bool`). Env overrides now bind to non-string
  fields of serde-backed config types (#1180).

### Fixed

- Apps booted by `TestApp` install the subscriber from the **test** profile
  (`application-test.yaml`), like the rest of their config — the macros used
  to install it from the default profile before the app was built.
- Logs emitted before the subscriber existed (`App::setup`, config loading,
  plugin `setup`) are no longer lost: they are buffered and replayed, or
  written to stderr when the boot fails first.

### Changed

- Built against `r2e-macros` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-macros/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-macros-v0.9.0/r2e-macros/CHANGELOG.md)).

## [0.8.0] - 2026-10-25

### Breaking

- The HTTP surface is `r2e_core::http::Routes<S>` (R2E's newtype) instead of
  `axum::Router<S>`: `AppBuilder::build()`, `Controller::routes`,
  `merge_router` / `register_routes` / `with_layer_fn`, plugin `add_layer` /
  `wrap_router` / `RoutesContext::register_routes`, `PreparedApp::routes()` /
  `routes_mut()` (were `router[_mut]`), `RunningApp::routes()`,
  `BootableApp::into_routes[_with_consumers]()` (were `into_router…`). The
  prelude exports `Routes` and `middleware::Next`, no longer `Router` /
  `from_fn`. `#[middleware(f, state)]` functions take the controller core as a
  plain leading `Arc<Self>` argument instead of `State<Arc<Self>>`. Built-in
  handlers (`/health`, request-id) use the new handler shape. Migration:
  `docs/migration/routes-newtype.md` (`plans/axum-containment-roadmap.md`,
  phase C).

### Fixed

- `dev-reload`: the dev headers middleware no longer rewrites `Connection` to
  `close` on a `101 Switching Protocols` response, which broke every `#[ws]`
  upgrade under `r2e dev` (the `ws` + `dev-reload` combination was not covered
  by CI; `cargo test -p r2e-core --all-features --test runtime` now exercises it).

### Changed

- `r2e-core` no longer depends on `tower-http`: `Cors`, `SecureHeaders` and
  `NormalizePath` reach it through `r2e_http::layers`, re-exported as
  `r2e_core::http::layers`. `runtime::layers::default_cors` /
  `normalize_path_router` (and the crate-root `default_cors`) are now
  re-exports of the `r2e-http` functions — same paths, same types. Apps that
  used `tower_http::timeout::TimeoutLayer` in `#[layer(..)]` /
  `.with_layer(..)` can drop their own `tower-http` dependency and write
  `r2e::http::layers::timeout::TimeoutLayer` (`plans/axum-containment-roadmap.md`,
  phase B).
- The single-listener `run()`, each sharded worker and `Running::serve_tracked`
  serve through `r2e_http::server::serve` instead of each assembling
  `into_make_service_with_connect_info` + `ListenerExt::tap_io` + `axum::serve`
  by hand. No behaviour change; `r2e_core::http::server` re-exports the module
  (`plans/axum-containment-roadmap.md`, phase A).

- Built against `r2e-http` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-http/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-http-v0.7.0/r2e-http/CHANGELOG.md)).
- Built against `r2e-macros` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-macros/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-macros-v0.8.0/r2e-macros/CHANGELOG.md)).

## [0.7.2] - 2026-10-24

### Changed

- No library change. The `controller` test target gains coverage for the
  `#[derive(Params)]` last-argument metadata (#1170) and for controller-level
  `#[middleware]` / `#[layer]` (#1074); the crate is re-cut so the published
  source matches the tagged tree (a release leaves nothing behind).

## [0.7.1] - 2026-10-15

### Fixed

- Requires `r2e-macros` 0.7.0, the first published macro crate whose
  `#[routes]` codegen matches the unboxed `__maybe_validate` of 0.7.0: 0.7.0
  resolved against `r2e-macros` 0.6.0 from crates.io, and every handler with
  validation failed to compile (`E0614`). No API change (#1092).

## [0.7.0] - 2026-10-12

### Breaking

- `Rejection` shrinks from 304 to 56 bytes (#1086): `details`, `headers` and
  the opaque response now live in a lazily allocated block, so
  `Result<T, Rejection>` and an envelope `enum E { Rejected(Rejection), … }`
  pass clippy's `result_large_err` / `large_enum_variant` without an
  `#[allow]`, in the framework and in applications. `kind`, `status`,
  `message` and `source` stay public fields. Read the cold members through
  `details()` / `take_details()` and `headers()` / `headers_mut()` /
  `take_headers()`; the `.details(value)` builder is now `.with_details(value)`
  (`details()` is the reader). `__maybe_validate` returns an unboxed
  `Result<(), Rejection>`. Migration: `docs/migration/rejection-layout.md`.
