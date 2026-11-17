# Changelog — r2e-devservices

All notable changes to `r2e-devservices` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the version is this
crate's own (pre-1.0: a breaking change bumps the minor, anything else the
patch).

Write entries under `[Unreleased]` in the PR that makes the change. A release
PR (`scripts/bump-version.sh`) turns that section into `[X.Y.Z] - date`, and
`.github/workflows/release.yml` publishes the crate, tags `r2e-devservices-vX.Y.Z` and
uses the section as the GitHub release notes.

Up to 0.6.0 the whole workspace was released in lockstep; that history lives
in the root [`CHANGELOG.md`](../CHANGELOG.md).

## [Unreleased]

### Breaking

- `DevKeycloak` default realm (#1424): `mcp-public` no longer allows the
  loopback redirect URIs `http://localhost:*` / `http://127.0.0.1:*`.
  Migration: an authorization-code flow redirecting to loopback uses the new
  client `mcp-local-cli` (below) instead of `mcp-public`; a custom realm
  passed to `DevKeycloak` is unaffected.

### Added

- `DevKeycloak` default realm (#1424): new public client `mcp-local-cli`
  (PKCE S256, loopback redirect URIs only, same `mcp` audience scope). Both
  `mcp-public` and `mcp-local-cli` allow direct access grants so
  `password_token` can mint a token with either `azp`.
