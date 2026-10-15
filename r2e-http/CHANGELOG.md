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

## [0.6.1] - 2026-10-15

### Changed

- No functional change: clippy's let-chain rewrites (`207f9d6b`) landed after
  0.6.0 was published, so the crates.io source no longer matched the tag.
  Re-released so the registry carries what the repository ships (#1092).
