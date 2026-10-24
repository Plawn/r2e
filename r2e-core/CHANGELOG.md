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
