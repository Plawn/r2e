# Changelog — r2e-grpc

All notable changes to `r2e-grpc` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the version is this
crate's own (pre-1.0: a breaking change bumps the minor, anything else the
patch).

Write entries under `[Unreleased]` in the PR that makes the change. A release
PR (`scripts/bump-version.sh`) turns that section into `[X.Y.Z] - date`, and
`.github/workflows/release.yml` publishes the crate, tags `r2e-grpc-vX.Y.Z` and
uses the section as the GitHub release notes.

Up to 0.6.0 the whole workspace was released in lockstep; that history lives
in the root [`CHANGELOG.md`](../CHANGELOG.md).

## [Unreleased]

## [0.10.0] - 2026-11-03

### Breaking

- gRPC identity rides the HTTP extractor contract (#1213). `GrpcIdentity` is
  removed: an `#[inject(identity)]` type is extracted through
  `FromRequestPartsVia<S, M>` (or `FromRequestParts`) against the app state,
  and its validator is a bean. The trait left behind is an empty
  `#[deprecated]` stub over an unsatisfiable sealed supertrait, so an old
  `impl GrpcIdentity for …` fails to build with an E0277 naming
  `docs/migration/grpc-identity.md` — even under `#[allow(deprecated)]`.
- `GrpcService<S, W>` is generic over the application state `S` and an
  inference-only witness `W` (like `McpService<S, W>`); `add_to_routes` takes
  `(routes, state: &S, ctx)`. A missing extractor bean (the
  `Arc<JwtClaimsValidator>` behind `AuthenticatedUser`) is a compile error at
  `register_grpc_service`. `AppBuilderGrpcExt` gains the witness parameter;
  `ModuleGrpcServices` implements `ModuleEndpoints<T, (W0, ..)>`.
- `guard_context(&tonic::Request, ..)` is removed: guards see a
  `GuardContext::from_parts` built over the per-call head.
- Missing credentials on an `AuthenticatedUser` identity answer
  `Unauthenticated` with the HTTP extractor's message (`"Unauthorized"`,
  previously `"Missing authorization metadata"`); a validator failure keeps
  `"Unauthorized"` but no longer carries `www-authenticate: Bearer` metadata
  (the HTTP extractor rejects with `HttpError` — known follow-up).

### Added

- `head` module: `into_head` / `from_head` turn a `tonic::Request` into an
  `http::request::Parts` head (POST, `/<service>/<method>` uri, metadata moved
  into the headers without copying, extensions moved — caller-owned entries
  kept —, `ConnectInfo` from the peer address unless one is already there) and
  back; no `MetadataMap` is deposited (an extractor wanting the tonic view
  builds `MetadataMap::from_headers(parts.headers.clone())`, paying the clone
  only where used); `CaptureMethodPath` is the routing layer that records the real
  method path (`MethodPath`). `#[grpc_routes]` services are wrapped in it.
- Struct-level `#[inject(identity)]` / `#[inject(request)]` fields and server
  streaming rpcs on `#[grpc_routes]` services (see `r2e-macros`).

## [0.9.0] - 2026-10-31

### Changed

- Built against `r2e-core` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-core/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-core-v0.9.0/r2e-core/CHANGELOG.md)).

## [0.8.0] - 2026-10-25

### Changed

- Built against the `Routes` newtype of `r2e-core` / `r2e-http` (breaking
  there: `docs/migration/routes-newtype.md`); this crate's own API is
  unchanged.

- Built against `r2e-core` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-core/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-core-v0.8.0/r2e-core/CHANGELOG.md)).

## [0.7.0] - 2026-10-12

### Changed

- Built against `r2e-core` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-core/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-core-v0.7.0/r2e-core/CHANGELOG.md)).

