# Changelog — r2e-http

All notable changes to `r2e-http` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the version is this
crate's own (pre-1.0: a breaking change bumps the minor, anything else the
patch).

Write entries under `[Unreleased]` in the PR that makes the change. A release
PR (`scripts/bump-version.sh`) turns that section into `[X.Y.Z] - date`, and
`.github/workflows/release.yml` publishes the crate, tags `r2e-http-vX.Y.Z` and
uses the section as the GitHub release notes.

Up to 0.6.0 the whole workspace was released in lockstep; that history lives
in the root [`CHANGELOG.md`](../CHANGELOG.md).

## [Unreleased]

### Breaking

- `Routes<S>` and `MethodRoutes<S>` (module `routing`, flattened at the crate
  root) replace the re-exported `axum::Router` / `MethodRouter`. They are
  newtypes with the same builder vocabulary and R2E-owned bounds (`Handler<S>`
  = `FnOnce(S, Request) -> Fut`, `RouteLayer`, `LeafService`); function
  middleware attaches through `middleware_fn` / `middleware_fn_with_state`
  instead of `middleware::from_fn[_with_state]`, which are no longer
  re-exported. `routing::Route` is R2E's own `tower::Service` newtype;
  `has_custom_fallback` became `Routes::has_custom_fallback(&self)`. axum's
  `serve` / `ListenerExt` are no longer re-exported (use `server::serve`).
  `axum_compat` gains `into_router` / `from_router` / `into_method_router` /
  `from_method_router`. Migration: `docs/migration/routes-newtype.md`
  (`plans/axum-containment-roadmap.md`, phase C).

### Added

- `r2e_http::layers` — the tower-http layers R2E uses, re-exported with their
  identity intact (`cors`, `timeout`, `set_header`, `normalize_path`), plus
  `default_cors()` (permissive) and `normalize_path_router()` (the pre-routing
  trailing-slash wrap), both moved here from `r2e-core`. tower-http is now a
  dependency of this crate; `tower` became unconditional (was `quic`-only).
  `plans/axum-containment-roadmap.md`, phase B.
- `r2e_http::server::{serve, ServeOptions}` — the one serve path. It takes the
  runtime facade's `TcpListener`, a `Router`, a shutdown future and the options,
  always installs `ConnectInfo<SocketAddr>`, applies `TCP_NODELAY` per accepted
  connection when asked, and returns the graceful-shutdown future. Every serve
  path in the workspace (single listener, sharded workers, `TestServer`) now
  goes through it, so the backend's serve API is named in this module only
  (`plans/axum-containment-roadmap.md`, phase A).

### Changed

- `r2e-rt` and `tracing` are now unconditional dependencies (they were only
  pulled in by the `quic` feature): `server::serve` needs the facade's listener
  type and logs a failed `TCP_NODELAY`.

## [0.6.1] - 2026-10-15

### Changed

- No functional change: clippy's let-chain rewrites (`207f9d6b`) landed after
  0.6.0 was published, so the crates.io source no longer matched the tag.
  Re-released so the registry carries what the repository ships (#1092).
