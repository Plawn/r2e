# r2e-tracing

Owner of the process-global `tracing` subscriber for R2E.

## Overview

A `tracing` subscriber is a one-shot process global: the first install wins.
`r2e-tracing` holds the knobs R2E installs it with and the install itself, so
every R2E entry point and plugin goes through one place:

- `TracingOptions` — filter, `LogFormat` (`Pretty` / `Json`), `SpanEvents` and
  the `fmt` layer toggles (target, thread ids/names, file, line number, level, ansi).
- `try_install(&TracingOptions)` — installs `Registry` + `fmt` layer + `EnvFilter`
  (`RUST_LOG` wins over `filter`); `Err(AlreadyInstalled)` when a subscriber is
  already in place.
- `installed()` — the options the winning install used, when R2E installed it.

The crate sits at the bottom of the R2E graph: no tokio, no `r2e-*` dependency.
Applications normally don't depend on it directly — `r2e-core` binds the YAML
`tracing:` section (`TracingConfig`) to `TracingOptions` and re-exports every
item (`r2e_core::LogFormat`, `r2e_core::r2e_tracing::try_install`, …).

`tracing_subscriber` is re-exported so `TracingOptions::effective_span_events()`
(`FmtSpan`) lines up with the version R2E builds against.
