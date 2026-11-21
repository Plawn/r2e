# Changelog — r2e-test

All notable changes to `r2e-test` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the version is this
crate's own (pre-1.0: a breaking change bumps the minor, anything else the
patch).

Write entries under `[Unreleased]` in the PR that makes the change. A release
PR (`scripts/bump-version.sh`) turns that section into `[X.Y.Z] - date`, and
`.github/workflows/release.yml` publishes the crate, tags `r2e-test-vX.Y.Z` and
uses the section as the GitHub release notes.

Up to 0.6.0 the whole workspace was released in lockstep; that history lives
in the root [`CHANGELOG.md`](../CHANGELOG.md).

## [Unreleased]

### Added

- `.path_param(name, value)` on `TestRequest` / `SessionRequest` fills a
  `{name}` placeholder of the request path (percent-encoded as one segment;
  `{*name}` keeps its `/` separators). An unknown name or an unfilled
  placeholder panics on `send()`.
- JSON paths project over arrays: `assert_json_path("users.name", json!(["Alice", "Bob"]))`.
  A field read on an array used to resolve to `null`.
- `TestResource`: an external dependency (database, broker, mock server)
  started before the boot, whose `config()` is applied to the builder.
  `TestResources` starts and holds them, `TestApp::with_resources` hands them
  to the app (dropped after it) and `app.resource::<R>()` reads one back.
  Declared with `resources(...)` on `#[r2e::test]` / `#[r2e::test_suite]`.
- `TestResource::configure(&self, builder) -> AppBuilder` (default: unchanged):
  a resource reaches the builder itself, typically to pin a bean it built —
  e.g. a rollback-only pool (`DbPool::rollback_only`) for per-test rollback.
  Runs after every resource's `config()` and before the `with = ...` hook.
- Feature `grpc`: `TestApp::grpc()` returns a `GrpcTestChannel` onto the gRPC
  services the app registered — `GreeterClient::new(app.grpc())` calls them
  in-process, with no port, in both transport modes. `.as_user(sub, roles)`,
  `.bearer(token)` and `.metadata(key, value)` set metadata on every call;
  `.service_names()` lists what is served.

## [0.11.0] - 2026-11-10

### Changed

- Built against `r2e-core` 0.10.0 — its breaking changes surface through this crate's re-exports (see [`r2e-core/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-core-v0.10.0/r2e-core/CHANGELOG.md)).
- Built against `r2e-security` 0.11.0 — its breaking changes surface through this crate's re-exports (see [`r2e-security/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-security-v0.11.0/r2e-security/CHANGELOG.md)).

## [0.10.0] - 2026-11-04

### Changed

- Built against `r2e-security` 0.10.0 — its breaking changes surface through this crate's re-exports (see [`r2e-security/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-security-v0.10.0/r2e-security/CHANGELOG.md)).

## [0.9.0] - 2026-10-31

### Changed

- `TestApp::boot` / `try_boot*` capture boot logs (`r2e_core::capture_boot_logs`)
  around `App::setup` and the build, and the subscriber is installed by
  `build_state()` from the app's **test** profile — `#[r2e::test]` no longer
  installs one (see `docs/migration/tracing-single-path.md`).

### Changed

- Built against `r2e-core` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-core/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-core-v0.9.0/r2e-core/CHANGELOG.md)).
- Built against `r2e-security` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-security/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-security-v0.9.0/r2e-security/CHANGELOG.md)).

## [0.8.0] - 2026-10-25

### Breaking

- `TestApp` wraps `r2e_core::http::Routes` instead of `axum::Router`
  (`TestApp::new(routes)`, `TestServer`); hand-assembled apps pass a `Routes`
  (see `docs/migration/routes-newtype.md`).

### Changed

- `TestServer::new` serves through `r2e_http::server::serve` like the booted-app
  form already did via `Running::serve_tracked`. Same `ConnectInfo<SocketAddr>`
  behaviour; no API change.

- Built against `r2e-core` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-core/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-core-v0.8.0/r2e-core/CHANGELOG.md)).
- Built against `r2e-security` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-security/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-security-v0.8.0/r2e-security/CHANGELOG.md)).

## [0.7.0] - 2026-10-12

### Changed

- Built against `r2e-core` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-core/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-core-v0.7.0/r2e-core/CHANGELOG.md)).
- Built against `r2e-security` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-security/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-security-v0.7.0/r2e-security/CHANGELOG.md)).

