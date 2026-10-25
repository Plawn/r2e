# Changelog — r2e-openapi

All notable changes to `r2e-openapi` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the version is this
crate's own (pre-1.0: a breaking change bumps the minor, anything else the
patch).

Write entries under `[Unreleased]` in the PR that makes the change. A release
PR (`scripts/bump-version.sh`) turns that section into `[X.Y.Z] - date`, and
`.github/workflows/release.yml` publishes the crate, tags `r2e-openapi-vX.Y.Z` and
uses the section as the GitHub release notes.

Up to 0.6.0 the whole workspace was released in lockstep; that history lives
in the root [`CHANGELOG.md`](../CHANGELOG.md).

## [Unreleased]

### Changed

- Built against the `Routes` newtype of `r2e-core` / `r2e-http` (breaking
  there: `docs/migration/routes-newtype.md`); this crate's own API is
  unchanged.

## [0.7.2] - 2026-10-24

### Changed

- No library change. The test suite gains the end-to-end check that a
  `#[derive(Params)]` last argument raises no `MissingRequestBody` warning
  (#1170); the crate is re-cut so the published source matches the tagged tree
  (a release leaves nothing behind).

## [0.7.1] - 2026-10-15

### Fixed

- Depends on `r2e-macros` 0.7.0 so a registry build resolves a single macro
  crate version next to `r2e-core` 0.7.1. No API change (#1092).

## [0.7.0] - 2026-10-12

### Changed

- Built against `r2e-core` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-core/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-core-v0.7.0/r2e-core/CHANGELOG.md)).

