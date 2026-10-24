# Changelog — r2e-rt

All notable changes to `r2e-rt` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the version is this
crate's own (pre-1.0: a breaking change bumps the minor, anything else the
patch).

Write entries under `[Unreleased]` in the PR that makes the change. A release
PR (`scripts/bump-version.sh`) turns that section into `[X.Y.Z] - date`, and
`.github/workflows/release.yml` publishes the crate, tags `r2e-rt-vX.Y.Z` and
uses the section as the GitHub release notes.

Up to 0.6.0 the whole workspace was released in lockstep; that history lives
in the root [`CHANGELOG.md`](../CHANGELOG.md).

## [Unreleased]

## [0.6.1] - 2026-10-24

### Added

- `task_local!` is re-exported (identity unchanged, like `select!`/`pin!`/`join!`),
  so an app can declare a request-context task-local as `r2e::rt::task_local!`
  without a direct `tokio` dependency (#1030).
