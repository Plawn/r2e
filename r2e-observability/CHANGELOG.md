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

## [0.9.0] - 2026-10-31

### Breaking

Migration guide: [`docs/migration/tracing-single-path.md`](../docs/migration/tracing-single-path.md).

- The plugin no longer builds nor installs a `tracing` subscriber. Its
  `setup` **contributes** two layers with `PluginSetupContext::tracing_layer`
  — the `tracing-opentelemetry` layer (`tracing_setup::otel_layer`) and
  `tracing_setup::OtelIds` (writes `trace_id`/`span_id` as `LogFields` on
  every exported span) — and `build_state()` installs them with R2E's one
  subscriber, configured by the app's top-level `tracing:` section. An app
  that installs its own subscriber (`tracing.enabled: false`) adds
  `otel_layer()` and `OtelIds` to its own `Registry`.
- `ObservabilityConfig.tracing` and the `observability.tracing.*` subscriber
  keys (`filter`, `format`, `ansi`, `span-events`, …) are removed: the console
  is configured by the top-level `tracing:` section only.
  `observability.tracing.enabled` (the OTLP export switch) stays. A leftover
  `observability.tracing.*` key is ignored and reported by one `ERROR` log at
  boot naming the migration guide; boot continues.
- `ObservabilityConfig::with_tracing_config`, `with_log_format` and
  `tracing_setup::TraceIdFormat` are deleted (the console is configured by
  `tracing:`, the ids are written by `OtelIds`).
- `tracing_setup::init_tracing` only builds the tracer provider; it no longer
  touches the global subscriber.
- `ObservabilityConfig` is `#[non_exhaustive]`: build it with `new`,
  `from_env` or `from_r2e_config` and the `with_*` methods.
- `app_main!(MyApp, tracing = false)` / `#[r2e::main(tracing = false)]` no
  longer compile (removed in `r2e`); `tracing.enabled: false` opts out of
  R2E's subscriber.

### Fixed

- `Suppress` layers contributed by other plugins (`McpServer`'s rmcp noise
  rules) now apply under this plugin — to the log lines and to the exported
  spans alike — because the plugin contributes to the one framework
  subscriber instead of building its own stack (#1185).
- Two `Observability` boots in one process (tests, dev-reload cycles) no
  longer lose OpenTelemetry export to the "subscriber already installed"
  race: the one contributed layer exports through the latest provider.

### Changed

- Built against `r2e-core` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-core/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-core-v0.9.0/r2e-core/CHANGELOG.md)).

## [0.8.0] - 2026-10-25

### Changed

- Built against `r2e-core` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-core/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-core-v0.8.0/r2e-core/CHANGELOG.md)).

## [0.7.0] - 2026-10-12

### Changed

- Built against `r2e-core` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-core/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-core-v0.7.0/r2e-core/CHANGELOG.md)).

