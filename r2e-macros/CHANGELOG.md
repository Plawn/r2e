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

### Breaking

- `#[r2e::main]`, `#[r2e::test]` and `#[r2e::test_suite]` no longer call
  `init_tracing_from_config()`, and their `tracing = …` argument is removed:
  writing it is a compile error naming the migration page. The subscriber is
  installed by `build_state()` from the `tracing:` section
  (`tracing.enabled: false` to install nothing). This is a change of the
  macro **syntax**, so it propagates as usual. Migration:
  `docs/migration/tracing-single-path.md`.
- `#[mcp_routes]` codegen targets the state-generic `McpService<S, W>` of
  `r2e-mcp` (`routes(state, ctx)`): it emits one `FromRequestPartsVia<S, M>`
  bound per member with an `#[inject(identity)]` parameter, folds the
  markers into the witness `W`, and runs the extractor on the call's request
  parts, mapping its `Rejection` into `McpError`. The macro syntax is
  unchanged, but this bump **propagates** (unlike a pure `r2e-core` codegen
  change): `#[mcp_routes]` reaches apps through the `r2e-core` prelude, so a
  patch re-pin of `r2e-core` would hand the new expansion to an app still on
  `r2e-mcp` 0.9 after a plain `cargo update` and break its build (#1092's
  shape). Migration: `docs/migration/mcp-identity.md` (#1175).

### Changed

- `#[r2e::test_suite]` installs the subscriber with `init_tracing_from_config()`
  (the `tracing:` section of `application.yaml`, built-in defaults when there
  is none) instead of `init_tracing()`, like `#[r2e::test]` and the entry
  points. Identical for a project without a `tracing:` section.

## [0.8.0] - 2026-10-25

### Breaking

- Generated code builds `Routes` / `MethodRoutes` and attaches
  `#[middleware]` through `MethodRoutes::middleware_fn[_with_state]`; a
  `#[middleware(f, state)]` function now receives `Arc<Self>` as its first
  argument instead of `State<Arc<Self>>`. Handler registration closures take
  `(state, Request)` explicitly — no backend routing type is named in the
  expansion. Migration: `docs/migration/routes-newtype.md`.

## [0.7.1] - 2026-10-24

### Added

- Controller-level `#[middleware]` / `#[layer]` (#1074): placed on the
  `#[routes]` impl block (below `#[routes]`), they wrap every route, `#[sse]`
  and `#[ws]` method of the controller, outside the method-level layers.
  `#[fallback]` routes are skipped (`Router::fallback` takes no per-route
  layers); an impl block with no other route is a dead-configuration compile
  error. `#[grpc_routes]` / `#[mcp_routes]` still reject both attributes.
- `#[middleware(fn, state)]` (method and impl level) emits
  `from_fn_with_state` with the controller core, so `fn` takes
  `State<Arc<Self>>`.
- `#[layer(|this: &Self| expr)]` (method and impl level) builds the layer from
  the controller core, once at route registration.

### Fixed

- Stacking two error-passthrough layers on one route (e.g. two
  `DefaultBodyLimit`s) no longer fails with E0284: every generated `.layer`
  call pins its error type to `Infallible`, which `Router::route` already
  required.
- A `#[derive(Params)]` struct as the last handler argument is no longer taken
  for an unmapped request body (#1170): `#[routes]` gates
  `request_body_unmapped` on an autoref probe for `ParamsMetadata`, so GET and
  POST routes ending in a Params struct stop logging the boot-time
  `MissingRequestBody` OpenAPI warning. A real body extractor without
  `RequestBodySchema` is still flagged.

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
