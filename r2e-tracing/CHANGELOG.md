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

## [0.1.1] - 2026-11-01

### Added

- `boot_logs_captured()` (#1192): whether `capture_boot_logs()` is buffering
  right now (the forwarder is ours, `install` has not run, nothing was
  discarded). `r2e-core`'s builder reads it before installing, to warn when
  nothing was capturing and the events logged before `build_state()` were
  dropped.

### Fixed

- The boot-log buffer honours `RUST_LOG` (#1192): what `capture_boot_logs()`
  keeps until `install` is decided by `RUST_LOG`, read when the capture
  starts, with its full directive syntax (`info,my_app=debug` buffers
  `my_app`'s DEBUG lines, not hyper's TRACE). DEBUG/TRACE lines from
  `App::setup`, config loading and plugin `setup` were dropped even when
  `RUST_LOG` asked for them. Unset or invalid `RUST_LOG` keeps INFO and above,
  as before; the 1024-event bound and the overflow WARN are unchanged. The
  replay still goes through the installed filter, so a buffered line that
  `tracing.filter` rejects is not printed.

## [0.1.0] - 2026-10-31

### Added

- New crate, extracted from `r2e-core`: it owns the process-global `tracing`
  subscriber. No tokio, no `r2e-*` dependency; `tracing_subscriber` is
  re-exported.
- Options: `LogFormat`, `SpanEvents` (moved from `r2e-core` as is), the plain
  options struct `TracingOptions` (same knobs and builders as
  `r2e_core::TracingConfig`, without the config binding) and the
  `#[non_exhaustive]` `JsonOptions` (`flatten`, `message_key`,
  `current_span`, `span_list`, with builders).
- `install(&TracingOptions, Vec<BoxedLayer>)`: the one install, called by the
  application builder. Builds `Registry` + the contributed layers + the
  console layer + `EnvFilter` as one plain `Layered` stack — contributed
  layers see every span and event, are reachable through
  `Dispatch::downcast_ref`, and may carry per-layer filters. `RUST_LOG` wins
  over `filter`; an invalid filter directive falls back to `info` and is
  logged at ERROR instead of failing the install. `installed()` returns the
  options the winning install used; `AlreadyInstalled` (with
  `changes_output`) reports a lost install — `installed: None` when a foreign
  subscriber won.
- Boot log capture: `capture_boot_logs()` installs a forwarding global
  subscriber that buffers INFO, WARN and ERROR events (at most 1024; one WARN
  reports how many were dropped) until `install`, which replays them in order
  through the real stack, rebuilds the callsite interest cache and forwards
  everything from then on. Spans opened before the install are safe to use
  and close after it. Dropping the `BootLogs` handle before any install (a
  failed boot) writes the buffered lines to stderr as
  `LEVEL target: message k=v`. `discard_boot_logs()` drops the buffer and
  disables every callsite (`tracing.enabled: false`). The `log` crate is
  bridged into `tracing`.
- `Suppress`: a layer dropping events with an exact level, target and message
  (`Suppress::new`, `Suppress::error`); contributed like any layer, it gates
  the whole stack through `event_enabled`.
- `LogFields`: a span extension (insertion-ordered key/value pairs) that the
  console layer appends to every line emitted in the span's scope, the
  closest span winning a key — ` key=value` in text, a string member in JSON,
  a top-level key in flat JSON.
- Console layer: text, nested JSON, and flat JSON lines (`LogFormat::Json` +
  `JsonOptions::flatten`) — event fields at the top level, the message under
  `JsonOptions::message_key` (the target when the event has none), the current
  span as `span` and the span stack (root first) as `spans`, RFC 3339
  `timestamp`, `level`, `target`, optional `thread_id`/`thread_name`/`file`/
  `line`; the formatter's keys win over colliding event and `LogFields` keys,
  and `log`-bridged events are normalized (`log.*` fields dropped). With
  `ansi` unset, colours are on only when stdout is a terminal.
