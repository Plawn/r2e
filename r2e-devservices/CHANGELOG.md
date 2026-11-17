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

### Changed

- `DevKeycloak` default realm (#1424): new public client `mcp-local-cli`
  (PKCE S256, loopback redirect URIs only, same `mcp` audience scope), and
  `mcp-public` no longer allows `http://localhost:*` / `http://127.0.0.1:*`
  — a browser flow redirecting to loopback must use `mcp-local-cli`. Both
  clients allow direct access grants so `password_token` can mint a token
  with either `azp`.
