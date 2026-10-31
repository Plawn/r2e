# Migration: one tracing path through the builder (r2e-core 0.9)

R2E used to have several ways to install the process-global `tracing`
subscriber — the entry-point macros (`app_main!`, `launch!`, `#[r2e::main]`,
`#[r2e::test]`, `#[r2e::test_suite]`), the `Tracing` / `ConfiguredTracing`
plugins, `init_tracing*`, `ensure_tracing_installed` — plus process-global
registries to extend it after the fact (`contribute_layer`, `suppress_error`,
`add_enricher`, `set_console_factory`). Whichever ran first won, silently.

Now there is **one** path: `build_state()` installs the subscriber, first
thing, from the loaded `tracing:` section plus every layer contributed with
`.tracing_layer(..)` before it. Everything the old entry points and registries
did is either automatic or a `tracing:` key or a layer.

Every leftover use of the old API **fails to build** with an error naming this
page (`r2e-compile-tests/cases/tracing/fail/*.rs` pin those diagnostics). Most
apps only delete lines.

## What changed

| Old | New |
|---|---|
| `app_main!(App, tracing = false)` / `launch!(App, tracing = false)` | `tracing.enabled: false` in `application.yaml` (or `R2E_TRACING_ENABLED=false`) |
| `#[r2e::main(tracing = …)]`, `#[r2e::test(tracing = …)]`, `#[r2e::test_suite(tracing = …)]` | delete the argument; `#[r2e::main]` / `#[r2e::test]` now only build the runtime (like `#[tokio::main]`) |
| `LaunchOptions { tracing, .. }` | gone: `LaunchOptions` has no field left (`#[non_exhaustive]`, `LaunchOptions::default()`) |
| `.plugin(Tracing)` / `.plugin(Tracing::from_config(&cfg))` / `.plugin(Tracing::configured(cfg))` / `.plugin(ConfiguredTracing(cfg))` | delete: `build_state()` installs from the `tracing:` section |
| `init_tracing()` / `init_tracing_from_config()` / `init_tracing_with_config(&cfg)` / `try_init_tracing_with_config(&cfg)` | delete in an app (`launch` / `TestApp::boot` + `build_state()` cover it); a test **without** an app that wants logs calls `r2e::r2e_tracing::install(&TracingOptions::default(), vec![])` |
| `ensure_tracing_installed()` (in a plugin) | delete; contribute layers with `ctx.tracing_layer(..)` in `setup` |
| `contribute_layer(Box::new(layer))` | `.tracing_layer(layer)` on the `AppBuilder`, or `ctx.tracing_layer(layer)` on `PluginSetupContext` — **before** `build_state()` |
| `suppress_error(target, message)` / `suppress(NoiseRule { .. })` / `NoiseFilter` / `is_suppressed` | `.tracing_layer(Suppress::error(target, message))` (or `Suppress::new(level, target, message)`) |
| `add_enricher(Box<dyn FieldEnricher>)` / `EnrichContext` | a layer that inserts `LogFields` on a span (`on_new_span` / `on_enter`); the console prints them on every line inside it |
| `set_console_factory(|opts| my_layer)` | `tracing.console: false` + `.tracing_layer(my_layer)` |
| `late_layers()` / `LateLayers` / `NotWired` on a hand-built `Registry` | `tracing.enabled: false` + add the layers you want (incl. `Suppress`) to your own `Registry` |
| `SubscriberAlreadyInstalled` | `AlreadyInstalled` (from `r2e-tracing`, re-exported) |
| `resolve_tracing_options()` / `tracing_options_of(&cfg)` | `TracingOptions::from(&TracingConfig)`; `installed()` returns what was installed |
| `TracingConfig { .. }` literal | `TracingConfig` is `#[non_exhaustive]`: `TracingConfig::default()` + `with_*`, or bind it from YAML |
| `TracingOptions` → `TracingConfig` conversion | gone; only `From<&TracingConfig> for TracingOptions` remains |
| `JsonLogConfig` read next to `TracingConfig` | a nested field: `TracingConfig.json` (`tracing.json.*`, unchanged keys) |
| `observability.tracing.filter` / `.format` / `.ansi` / … | top-level `tracing.*` (a leftover key is ignored and reported by one ERROR at boot) |
| `ObservabilityConfig::with_log_format` / `with_tracing_config`, `tracing_setup::TraceIdFormat` | deleted: `tracing.*` configures the console; `OtelIds` writes `trace_id` / `span_id` |
| `use r2e::prelude::ConfiguredTracing` | removed from the prelude |

New: `AppBuilder::tracing_layer`, `PluginSetupContext::tracing_layer`,
`tracing.enabled`, `tracing.console`, and the boot-log buffer (below).
Re-exported at the `r2e` / `r2e_core` root: `Suppress`, `LogFields`,
`BoxedLayer`, `TracingOptions`, `JsonOptions`, `LogFormat`, `SpanEvents`,
`TracingConfig`, `JsonLogConfig`, `capture_boot_logs`, `BootLogs`,
`installed`, `AlreadyInstalled`, and the crate itself as `r2e::r2e_tracing`.

## Step by step

1. **Entry point.** Remove every `tracing = …` argument (`app_main!`,
   `launch!`, `#[r2e::main]`, `#[r2e::test]`, `#[r2e::test_suite]`) and every
   `LaunchOptions { tracing: .. }`. If the argument was `false` because the app
   installs its own subscriber, see step 4.

2. **Plugins and calls.** Delete `.plugin(Tracing…)`, `ConfiguredTracing(..)`,
   `init_tracing*()` and `ensure_tracing_installed()`. Move the settings they
   carried to the `tracing:` section:

   ```yaml
   tracing:
     enabled: true          # false: R2E installs nothing (you own the subscriber)
     console: true          # false: no console layer; contributed layers still run
     filter: "info,my_app=debug"   # RUST_LOG wins when set
     format: json           # pretty | json (default pretty)
     target: true
     thread-ids: false
     thread-names: false
     file: false
     line-number: false
     level: true
     ansi: true             # unset: on only when stdout is a terminal
     span-events: close     # none | new | close | active | full (default close)
     json:
       flatten: true        # one flat object per line (VictoriaLogs / Loki)
       message-key: _msg    # default "message"
       current-span: true
       span-list: true
   ```

   Every key also binds from the environment (`R2E_TRACING_ENABLED=false`,
   `R2E_TRACING_FORMAT=json`, `R2E_TRACING_JSON_FLATTEN=true`). An invalid
   section does not abort the boot: the built-in defaults are installed and
   one ERROR names the problem.

3. **Extension points.** Turn each registry call into a layer contributed
   before `build_state()`:

   ```rust,ignore
   use r2e::prelude::*;
   use r2e::{LogFields, Suppress};

   // app code
   AppBuilder::new()
       .load_config::<()>()
       .tracing_layer(Suppress::error("some_dep::conn", "connection reset during shutdown"))
       .tracing_layer(RegionFields)          // writes LogFields on spans
       .build_state()
       .await;

   // plugin code
   impl Plugin for MyPlugin {
       // ...
       fn setup(&mut self, ctx: &mut PluginSetupContext) {
           ctx.tracing_layer(Suppress::error("noisy_lib", "harmless noise"));
       }
   }
   ```

   A field enricher becomes a layer writing `LogFields` (closest span wins on
   a key collision):

   ```rust,ignore
   use r2e::r2e_tracing::tracing_subscriber::{layer::Context, registry::LookupSpan, Layer};

   struct RegionFields;

   impl<S: tracing::Subscriber + for<'a> LookupSpan<'a>> Layer<S> for RegionFields {
       fn on_new_span(&self, _: &tracing::span::Attributes<'_>, id: &tracing::span::Id, ctx: Context<'_, S>) {
           if let Some(span) = ctx.span(id) {
               let mut fields = LogFields::new();
               fields.insert("region", "eu-west-1");
               span.extensions_mut().insert(fields);
           }
       }
   }
   ```

   A custom console (formerly `set_console_factory`): set
   `tracing.console: false` and contribute your `fmt` layer —
   `.tracing_layer(fmt::layer().compact())`. The global filter still applies
   to it.

4. **Own subscriber.** An app that builds its own `tracing_subscriber`
   stack sets `tracing.enabled: false`. R2E then installs nothing, drops the
   boot-log buffer, and ignores contributed layers — add the ones you need
   (`Suppress`, `r2e_observability::tracing_setup::otel_layer()` +
   `OtelIds`, …) to your own `Registry` directly.

## The foreign-subscriber rule

The global subscriber is one-shot. If something installs one **before**
`build_state()` (the app's `main`, a library, a test harness) and
`tracing.enabled` is not `false`:

- that subscriber wins and keeps receiving every event;
- the layers contributed with `.tracing_layer(..)` are dropped, and
  `build_state()` logs **one WARN** naming how many
  (`set tracing.enabled: false to silence`);
- `r2e::installed()` returns `None` (R2E installed nothing).

A **second** `build_state()` in a process where R2E already installed (a
second `TestApp::boot`, dev-reload cycles 2+) keeps the first subscriber;
its contributed layers are dropped with one DEBUG line. The
`Observability` OTel layer is unaffected: it always exports through the latest
provider.

## Boot logs

The subscriber is only known once `build_state()` has read `tracing:` and
every plugin has contributed — after `App::setup` and config loading have
already logged. `r2e::launch` / `app_main!` / `TestApp::boot` therefore call
`capture_boot_logs()` first: a forwarder buffers what is logged before the
install and replays it through the real subscriber once installed. Limits:

- **INFO and above only**: DEBUG / TRACE emitted before the install are lost.
- **No spans**: a replayed event has no parent span (no `LogFields` either).
- **Replay timestamps**: replayed lines are timestamped at replay time (the
  gap is `setup` + plugin `setup`, typically milliseconds).
- **Bounded at 1024 events**; the overflow is counted and reported by one WARN
  after the replay.
- If the boot fails **before** the install (config unreadable, …), dropping
  the `BootLogs` handle writes the buffered lines to stderr, ahead of the boot
  error report.
- A hand-written `main` that calls `build_state()` without `launch` gets no
  buffer: what it logs before `build_state()` goes nowhere. Call
  `let _boot = r2e::capture_boot_logs();` first to keep it.

## Tests

- `#[r2e::test]` / `#[r2e::test_suite]` no longer install anything. A test
  that boots the app (`TestApp::boot`, `#[r2e::test(app = ..)]`) gets the
  subscriber from the **test** profile (`application-test.yaml`), like any
  other config.
- A test **without** an app that wants log output installs it once:

  ```rust,ignore
  let _ = r2e::r2e_tracing::install(&r2e::TracingOptions::default(), vec![]);
  ```

- Quiet a whole test binary with `tracing: { enabled: false }` in
  `application-test.yaml`.
- Asserting on log lines: install a capturing layer with
  `tracing::subscriber::set_default(Registry::default().with(capture).with(Suppress::error(..)))`
  in the test (thread-local, no global state), or contribute it with
  `AppBuilder::tracing_layer` in a test binary of its own (the global
  subscriber is one-shot per process).

## Recipe: an app that had its own JSON subscriber

Before — the entry point opted out and `main` built a flat-JSON stack for
VictoriaLogs:

```rust,ignore
r2e::app_main!(MyApp, tracing = false);
// App::setup: my_logging::init_json_subscriber("info")?;
```

After — delete both lines' worth of code and configure it:

```rust,ignore
r2e::app_main!(MyApp);
```

```yaml
tracing:
  format: json
  filter: "info"
  json:
    flatten: true
    message-key: _msg
```

A field the custom formatter added on every line (service name, region)
becomes a `LogFields` layer contributed with `.tracing_layer(..)`; a dropped
noisy event becomes a `Suppress`.
