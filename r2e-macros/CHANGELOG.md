# Changelog — r2e-macros

All notable changes to `r2e-macros` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the version is this
crate's own (pre-1.0: a breaking change bumps the minor, anything else the
patch).

Write entries under `[Unreleased]` in the PR that makes the change. A release
PR (`scripts/bump-version.sh`) turns that section into `[X.Y.Z] - date`, and
`.github/workflows/release.yml` publishes the crate, tags `r2e-macros-vX.Y.Z` and
uses the section as the GitHub release notes.

Up to 0.6.0 the whole workspace was released in lockstep; that history lives
in the root [`CHANGELOG.md`](../CHANGELOG.md).

## [Unreleased]

## [0.7.0] - 2026-10-15

### Breaking

- The generated code targets `r2e-core` ≥ 0.7.1 and `r2e-mcp` ≥ 0.7.0 (#1092):
  `#[routes]` passes on the `Rejection` that `__maybe_validate` returns unboxed
  since r2e-core 0.7.0 (#1086) instead of dereferencing a `Box`, and
  `#[mcp_routes]` fills the `meta` field of `ToolRoute` / `ResourceRoute` /
  `PromptRoute` (r2e-mcp 0.7.0). Both changes were in the repository when those
  crates shipped, but `r2e-macros` 0.6.0 on crates.io predates them, so `r2e`
  0.7.0 / 0.8.0 taken from the registry failed on every `#[routes]` handler
  with validation (`E0614: type \`Rejection\` cannot be dereferenced`) and
  every `#[mcp_routes]` impl (missing field `meta`). The direct consumers
  re-pin this version in the same release: `r2e-core` 0.7.1, `r2e-executor`
  0.7.1, `r2e-openapi` 0.7.1, `r2e` 0.8.1.

### Added

- `#[tool(meta = …)]`, `#[resource(meta = …)]`, `#[prompt(meta = …)]`: an
  expression `Into<MetaObject>` (typically `meta!({ … })`) advertised as the
  member's `_meta` (r2e-mcp 0.7.0, task #1073).

### Changed

- `#[bean]` emits `#[allow(clippy::new_without_default)]` on a zero-argument
  constructor: the bean graph calls it, so users no longer need a `Default`
  impl (or a hand-written `allow`) to keep clippy quiet.
