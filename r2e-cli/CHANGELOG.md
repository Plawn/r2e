# Changelog — r2e-cli

All notable changes to `r2e-cli` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the version is this
crate's own (pre-1.0: a breaking change bumps the minor, anything else the
patch).

Write entries under `[Unreleased]` in the PR that makes the change. A release
PR (`scripts/bump-version.sh`) turns that section into `[X.Y.Z] - date`, and
`.github/workflows/release.yml` publishes the crate, tags `r2e-cli-vX.Y.Z` and
uses the section as the GitHub release notes.

Up to 0.6.0 the whole workspace was released in lockstep; that history lives
in the root [`CHANGELOG.md`](../CHANGELOG.md).

## [Unreleased]

## [0.9.0] - 2026-10-25

### Changed

- Built against the `Routes` newtype of `r2e-core` / `r2e-http` (breaking
  there: `docs/migration/routes-newtype.md`); this crate's own API is
  unchanged.

- Tracks `r2e` 0.9.0: `r2e new`, `r2e add` and `r2e docs --llm` target that facade version.

## [0.8.1] - 2026-10-15

### Changed

- Tracks `r2e` 0.8.1: `r2e new`, `r2e add` and `r2e docs --llm` target that facade version.

## [0.8.0] - 2026-10-12

### Changed

- Tracks `r2e` 0.8.0: `r2e new`, `r2e add` and `r2e docs --llm` target that facade version.

## [0.7.0] - 2026-10-11

### Changed

- Tracks `r2e` 0.7.0: `r2e new`, `r2e add` and `r2e docs --llm` target that facade version.

