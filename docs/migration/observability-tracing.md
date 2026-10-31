# Migration: `Observability` contributes to R2E's subscriber

`r2e-observability` 0.9.0 stops building a `tracing` subscriber. The
`Observability` plugin now **contributes** to the one subscriber R2E installs
from the app's top-level `tracing:` section:

- the `tracing-opentelemetry` layer goes into the subscriber's late-layer slot
  (`r2e_core::contribute_layer`) — spans are exported, `Span::current().context()`
  works, and R2E's noise rules gate what reaches OpenTelemetry too;
- a field enricher (`r2e_core::add_enricher`) adds `trace_id` / `span_id` to
  every console line emitted inside a traced span.

The console format, filter and ANSI settings therefore live in one place — the
`tracing:` section — whether or not the plugin is installed.

## What changed

| Before | After |
|---|---|
| `observability.tracing.filter`, `.format`, `.ansi`, `.span-events`, … | top-level `tracing.filter`, `tracing.format`, … |
| `observability.tracing.enabled` | unchanged — still the OTLP export switch |
| `ObservabilityConfig::with_log_format(LogFormat::Json)` | delete; `tracing.format: json` in YAML (or `Tracing::configured(..)`) |
| `ObservabilityConfig::with_tracing_config(cfg)` | delete; move `cfg` to the `tracing:` section |
| `ObservabilityConfig { tracing, .. }` / `config.tracing` | gone; `ObservabilityConfig` is `#[non_exhaustive]` — use `new` / `from_env` / `from_r2e_config` + `with_*` |
| `app_main!(MyApp, tracing = false)` so the plugin could own the subscriber | plain `app_main!(MyApp)` |
| "Observability tracing layer skipped" warning, no spans exported | gone: the plugin works whichever side installed the subscriber |
| `tracing_setup::TraceIdFormat` | deprecated: the enricher replaces it |

## Step by step

1. **YAML.** Move every `observability.tracing.*` key except `enabled` to the
   top-level `tracing:` section:

   ```yaml
   # before
   observability:
     tracing:
       enabled: true
       format: json
       filter: "info,my_app=debug"

   # after
   observability:
     tracing:
       enabled: true
   tracing:
     format: json
     filter: "info,my_app=debug"
   ```

   A key left behind does not fail the boot: it is ignored, and one `ERROR`
   names it at startup (`observability.tracing.* no longer configures the log
   subscriber and these keys are ignored`). Fix it the same way.

2. **Rust.** Delete `.with_log_format(..)` / `.with_tracing_config(..)` calls on
   `ObservabilityConfig`. They now fail to compile with an error naming this
   page. A struct literal or a `config.tracing` access fails with an ordinary
   "no field" error.

3. **Entry point.** Remove `tracing = false` from `app_main!`, `launch!` or
   `#[r2e::main]` if you only added it for `Observability`. R2E installs the
   subscriber from `tracing:`; the plugin contributes its layer to it. (The
   knob still exists for apps that install a subscriber of their own — see 4.)

4. **Own subscriber.** If the app installs its own subscriber (`tracing = false`
   plus a hand-built `tracing_subscriber::registry()`), it must host the
   late-layer slot, **first, directly on the `Registry`** — otherwise no span
   is exported and the plugin logs one `WARN` saying so:

   ```rust,ignore
   use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

   tracing_subscriber::registry()
       .with(r2e::late_layers())
       .with(my_fmt_layer)
       .with(r2e::NoiseFilter)
       .init();
   ```

   Field enrichers (the `trace_id`/`span_id` log fields) only apply to R2E's
   console layer; a hand-built fmt layer does not get them.

5. **`TraceIdFormat`.** If you used it as an event formatter, drop it: under
   R2E's subscriber the ids are added by the plugin's enricher, in both text
   and JSON. It stays (deprecated) for hand-built stacks until its removal.
