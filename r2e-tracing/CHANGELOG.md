# Changelog — r2e-tracing

All notable changes to `r2e-tracing` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the version is this
crate's own (pre-1.0: a breaking change bumps the minor, anything else the
patch).

Write entries under `[Unreleased]` in the PR that makes the change. A release
PR (`scripts/bump-version.sh`) turns that section into `[X.Y.Z] - date`, and
`.github/workflows/release.yml` publishes the crate, tags `r2e-tracing-vX.Y.Z`
and uses the section as the GitHub release notes.

## [Unreleased]

### Added

- New crate, extracted from `r2e-core`: it owns the process-global `tracing`
  subscriber. `LogFormat`, `SpanEvents` (moved as is) and the new plain
  options struct `TracingOptions` (same knobs and builders as
  `r2e_core::TracingConfig`, without the config binding); `try_install` (the
  body of `r2e_core::try_init_tracing_with_config`), `installed()` (the options
  the winning install used) and `AlreadyInstalled` (with `changes_output`).
  `tracing_subscriber` is re-exported. No tokio, no `r2e-*`
  dependency. `r2e-core` re-exports every item at its previous path.
- Noise rules: `suppress(NoiseRule)` / `suppress_error(target, message)`
  register a process-wide, append-only rule (exact level + target + message;
  empty by default), `is_suppressed` queries it, and `NoiseFilter` — usable as
  a global `Layer` or a per-layer `Filter` — drops the matched events. The
  installed subscriber carries it above the console and late layers.
- Late-layer slot: `contribute_layer(BoxedLayer)` adds a layer to the
  installed subscriber after the fact (`Err(NotWired)` when no subscriber
  hosts the slot); `late_layers()` / `LateLayers` puts the slot in a
  hand-built stack. Contributed layers see spans and events from then on and
  are reachable through `Dispatch::downcast_ref`; they cannot carry a
  per-layer filter.
- Console field enrichers: `add_enricher(Box<dyn FieldEnricher>)` appends
  fields to every console line (` key=value` in text, string members in JSON),
  with `EnrichContext` describing the event.

### Changed

- An invalid filter directive no longer panics `try_install`: the subscriber
  is installed with `info` and the parse error is logged at ERROR through it.
- With `ansi` unset, the console layer colours its output only when stdout is
  a terminal.
