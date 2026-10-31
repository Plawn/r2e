# Changelog — r2e-observability

All notable changes to `r2e-observability` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the version is this
crate's own (pre-1.0: a breaking change bumps the minor, anything else the
patch).

Write entries under `[Unreleased]` in the PR that makes the change. A release
PR (`scripts/bump-version.sh`) turns that section into `[X.Y.Z] - date`, and
`.github/workflows/release.yml` publishes the crate, tags `r2e-observability-vX.Y.Z` and
uses the section as the GitHub release notes.

Up to 0.6.0 the whole workspace was released in lockstep; that history lives
in the root [`CHANGELOG.md`](../CHANGELOG.md).

## [Unreleased]

### Breaking

Migration guide: [`docs/migration/observability-tracing.md`](../docs/migration/observability-tracing.md).

- The plugin no longer builds a `tracing` subscriber. Its `setup` makes sure
  R2E's one subscriber is installed (`r2e_core::ensure_tracing_installed`,
  from the app's top-level `tracing:` section), then **contributes** to it:
  the `tracing-opentelemetry` layer through `r2e_core::contribute_layer` and
  the `trace_id`/`span_id` log fields through `r2e_core::add_enricher`. An app
  that installs its own subscriber must add `r2e_core::late_layers()` first on
  its `Registry`, or no span is exported (one `WARN` says so).
- `ObservabilityConfig.tracing` and the `observability.tracing.*` subscriber
  keys (`filter`, `format`, `ansi`, `span-events`, …) are removed: the console
  is configured by the top-level `tracing:` section only.
  `observability.tracing.enabled` (the OTLP export switch) stays. A leftover
  `observability.tracing.*` key is ignored and reported by one `ERROR` log at
  boot naming the migration guide; boot continues.
- `ObservabilityConfig::with_tracing_config` and `with_log_format` fail to
  compile, with an error naming the migration guide.
- `ObservabilityConfig` is `#[non_exhaustive]`: build it with `new`,
  `from_env` or `from_r2e_config` and the `with_*` methods.
- `tracing_setup::TraceIdFormat` is deprecated (the enricher replaces it).
- `app_main!(MyApp, tracing = false)` / `#[r2e::main(tracing = false)]` is no
  longer needed for OTLP export (it still opts out of R2E's subscriber).

### Fixed

- R2E's noise rules (`r2e_core::NoiseFilter`) now apply under this plugin —
  to the log lines and to the exported spans alike — because the plugin
  contributes to the one framework subscriber, which carries the filter,
  instead of building its own stack: with `McpServer`, rmcp ≤ 3.8's drain
  ERROR after a stateless MCP client disconnect no longer reaches the logs
  nor the exported spans (#1185).
- Two `Observability` boots in one process (tests, dev-reload cycles) no
  longer lose OpenTelemetry export to the "subscriber already installed"
  race: the one contributed layer exports through the latest provider.

## [0.8.0] - 2026-10-25

### Changed

- Built against `r2e-core` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-core/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-core-v0.8.0/r2e-core/CHANGELOG.md)).

## [0.7.0] - 2026-10-12

### Changed

- Built against `r2e-core` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-core/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-core-v0.7.0/r2e-core/CHANGELOG.md)).

