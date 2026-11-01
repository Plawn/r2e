# r2e-tracing

Owner of the process-global `tracing` subscriber for R2E.

## Overview

A `tracing` subscriber is a one-shot process global: the first install wins.
R2E installs it once, from the application builder, through this crate:

- `TracingOptions` — filter, `LogFormat` (`Pretty` / `Json`), `JsonOptions`
  (flat JSON lines), `SpanEvents` and the `fmt` layer toggles (target, thread
  ids/names, file, line number, level, ansi).
- `install(&TracingOptions, Vec<BoxedLayer>)` — installs `Registry` + the
  contributed layers + the console layer + `EnvFilter` (`RUST_LOG` wins over
  `filter`); `Err(AlreadyInstalled)` when a subscriber is already in place.
- `installed()` — the options the winning install used, when R2E installed it.
- `capture_boot_logs()` — buffers the events emitted before `install` (what
  `RUST_LOG` enables, INFO+ without it) and replays them through the real
  stack and its filter (or writes them to stderr when the
  boot fails first); `discard_boot_logs()` turns logging off instead.
- `Suppress` — a layer dropping one exact (level, target, message) event.
- `LogFields` — a span extension whose fields the console prints on every
  line inside the span.

The crate sits at the bottom of the R2E graph: no tokio, no `r2e-*` dependency.
Applications normally don't depend on it directly — `r2e-core` binds the YAML
`tracing:` section to `TracingOptions`, calls `install` from the builder and
re-exports the items applications need.

`tracing_subscriber` is re-exported so `TracingOptions::effective_span_events()`
(`FmtSpan`) and contributed layers line up with the version R2E builds against.
