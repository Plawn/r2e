# Changelog — r2e-security

All notable changes to `r2e-security` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the version is this
crate's own (pre-1.0: a breaking change bumps the minor, anything else the
patch).

Write entries under `[Unreleased]` in the PR that makes the change. A release
PR (`scripts/bump-version.sh`) turns that section into `[X.Y.Z] - date`, and
`.github/workflows/release.yml` publishes the crate, tags `r2e-security-vX.Y.Z` and
uses the section as the GitHub release notes.

Up to 0.6.0 the whole workspace was released in lockstep; that history lives
in the root [`CHANGELOG.md`](../CHANGELOG.md).

## [Unreleased]

### Breaking

- `extractor::extract_jwt_claims`, `extract_jwt_claims_as` and
  `extract_jwt_identity` return `Result<_, r2e_core::Rejection>` instead of
  `Result<_, HttpError>` (#1173), and the `FromRequestPartsVia` /
  `OptionalFromRequestPartsVia` impls of `AuthenticatedUser` and of every
  `impl_claims_identity_extractor!` identity declare `type Rejection =
  Rejection`. A `?` into an `HttpError`-returning function still compiles
  (`From<Rejection> for HttpError`) **but drops the challenge again**, since
  `HttpError` carries no headers: a custom extractor built on these helpers
  must reject with `Rejection` to keep it —

  ```rust,ignore
  // before
  type Rejection = HttpError;
  async fn from_request_parts(..) -> Result<Self, HttpError> {
      let claims = extract_jwt_claims(parts, state).await?; // 401, no challenge
  // after
  type Rejection = Rejection;
  async fn from_request_parts(..) -> Result<Self, Rejection> {
      let claims = extract_jwt_claims(parts, state).await?; // 401 + WWW-Authenticate
  ```

  Code matching on the `HttpError` variants of these results switches to
  `rejection.kind` / `rejection.status`. `FromValidatedJwtClaims::from_jwt_claims`
  keeps returning `HttpError` (it runs after validation; no challenge applies).

### Fixed

- `AuthenticatedUser` (and every `impl_claims_identity_extractor!` identity)
  carries `WWW-Authenticate` on its 401 again (#1173, lost since #1072): a
  bare `Bearer` when no credentials were presented, `Bearer
  error="invalid_token", error_description="…"` when a token was refused
  (RFC 6750 §3) — `SecurityError::challenge()` is the single source. The
  header rides the `Rejection`, so it survives `#[routes]` projection through
  an error envelope (unless the envelope's own response sets
  `WWW-Authenticate`: the envelope wins per header name) and reaches gRPC
  metadata through `rejection_to_status`.

## [0.10.0] - 2026-11-04

### Breaking

- Feature `grpc` (#1213): `JwtIdentitySpec` and the `GrpcIdentity` impl of
  `AuthenticatedUser` are removed — gRPC extracts `AuthenticatedUser` through
  its HTTP `FromRequestPartsVia` impl (the `Arc<JwtClaimsValidator>` bean).
  The feature keeps `impl JwtClaimsValidatorLike for JwtClaimsValidator` for
  the manual metadata helpers. Migration: `docs/migration/grpc-identity.md`.

### Added

- `SecurityError::kind() -> SecurityErrorKind`: the finer category of a
  failure (`MissingAuthHeader`, `InvalidAuthScheme`, `MalformedToken`,
  `InvalidSignature`, `TokenExpired`, `MissingKid`, `UnknownKid`,
  `JwksUnavailable`, `AlgorithmRefused`, `SigningKeyUnusable`, `Claims`) for
  logging and metrics, without parsing the message. The enum is
  `#[non_exhaustive]`; the messages of `InvalidToken` / `ValidationFailed`
  are unchanged apart from a stable prefix on the algorithm and signing-key
  failures.

## [0.9.0] - 2026-10-31

### Changed

- Built against `r2e-core` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-core/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-core-v0.9.0/r2e-core/CHANGELOG.md)).
- Built against `r2e-grpc` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-grpc/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-grpc-v0.9.0/r2e-grpc/CHANGELOG.md)).

## [0.8.0] - 2026-10-25

### Changed

- Built against `r2e-core` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-core/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-core-v0.8.0/r2e-core/CHANGELOG.md)).
- Built against `r2e-grpc` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-grpc/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-grpc-v0.8.0/r2e-grpc/CHANGELOG.md)).

## [0.7.0] - 2026-10-12

### Changed

- Built against `r2e-core` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-core/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-core-v0.7.0/r2e-core/CHANGELOG.md)).
- Built against `r2e-grpc` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-grpc/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-grpc-v0.7.0/r2e-grpc/CHANGELOG.md)).

