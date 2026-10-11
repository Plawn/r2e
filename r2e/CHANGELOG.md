# Changelog — r2e

All notable changes to `r2e` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the version is this
crate's own (pre-1.0: a breaking change bumps the minor, anything else the
patch).

Write entries under `[Unreleased]` in the PR that makes the change. A release
PR (`scripts/bump-version.sh`) turns that section into `[X.Y.Z] - date`, and
`.github/workflows/release.yml` publishes the crate, tags `r2e-vX.Y.Z` and
uses the section as the GitHub release notes.

Up to 0.6.0 the whole workspace was released in lockstep; that history lives
in the root [`CHANGELOG.md`](../CHANGELOG.md).

## [Unreleased]

## [0.7.0] - 2026-10-11

### Breaking

- `r2e::mcp` (feature `mcp`) re-exports `r2e-mcp` 0.7: the MCP endpoint is
  always mounted and every family always advertised, `McpServer::Provided` is
  a 4-tuple, `McpSessionError` → `McpMemberError` (alias kept), the route
  structs gain `meta`. Migration: [`docs/migration/mcp-registry.md`](../docs/migration/mcp-registry.md);
  details in [`r2e-mcp/CHANGELOG.md`](../r2e-mcp/CHANGELOG.md).

### Added

- `r2e::prelude` (feature `mcp`) re-exports `McpRegistry`, the `meta!` macro
  and `MetaObject`.

### Changed

- Built against `r2e-mcp` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-mcp/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-mcp-v0.7.0/r2e-mcp/CHANGELOG.md)).

