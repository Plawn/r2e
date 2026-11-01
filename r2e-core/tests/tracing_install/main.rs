//! `build_state()` installs the process-global `tracing` subscriber, first
//! thing, from the `tracing:` section plus every layer contributed with
//! `.tracing_layer(..)` — on the builder and from a plugin's `setup`. With a
//! boot-log capture in place (what `launch` / `TestApp::boot` do), what was
//! logged before is replayed, the install is summarised in one INFO line
//! (`source=config`), and no "no boot-log capture" WARN is emitted.
//!
//! Its **own target**: the global subscriber is a one-shot process global, so
//! who wins the install is a property of the whole binary. One test drives the
//! sequence here, in order.

use std::sync::{Arc, Mutex};

use r2e_core::config::R2eConfig;
use r2e_core::plugin::{Plugin, PluginBuildContext, PluginBuildError, PluginSetupContext};
use r2e_core::{capture_boot_logs, installed, AppBuilder, LogFields, Suppress};
use tracing::span::{Attributes, Id};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

/// One captured event: `"LEVEL target message [k=v ...]"`, the `LogFields`
/// of its span scope appended.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<String>>>);

impl Capture {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

impl<S> Layer<S> for Capture
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        struct Message(String);
        impl tracing::field::Visit for Message {
            fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
                if f.name() == "message" {
                    self.0 = format!("{v:?}");
                }
            }
        }
        let mut message = Message(String::new());
        event.record(&mut message);
        let meta = event.metadata();
        let mut line = format!("{} {} {}", meta.level(), meta.target(), message.0);
        if let Some(scope) = ctx.event_scope(event) {
            for span in scope {
                if let Some(fields) = span.extensions().get::<LogFields>() {
                    for (k, v) in fields.iter() {
                        line.push_str(&format!(" {k}={v}"));
                    }
                }
            }
        }
        self.0.lock().unwrap().push(line);
    }
}

/// Writes `LogFields { request = <span name> }` on every new span — what an
/// OpenTelemetry id layer does with trace/span ids.
struct SpanNames;

impl<S> Layer<S> for SpanNames
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("the span is new");
        let mut fields = LogFields::new();
        fields.insert("request", attrs.metadata().name());
        span.extensions_mut().insert(fields);
    }
}

/// Contributes its layers from `setup`, like `McpServer` / `Observability`.
struct NoisyLib;

impl Plugin for NoisyLib {
    type Provided = ();
    type Deps = ();
    type Config = ();
    type Controllers = ();

    fn setup(&mut self, ctx: &mut PluginSetupContext) {
        ctx.tracing_layer(Suppress::error("noisy_lib", "harmless noise"));
        ctx.tracing_layer(SpanNames);
    }

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        _ctx: &mut PluginBuildContext,
    ) -> Result<(), PluginBuildError> {
        Ok(())
    }
}

#[r2e_core::test]
async fn build_state_installs_the_section_and_the_contributed_layers() {
    // SAFETY: the only test of the binary, before anything reads the
    // environment. `RUST_LOG` would override the filter asserted below.
    unsafe { std::env::remove_var("RUST_LOG") };
    assert_eq!(installed(), None, "nothing installed before build_state");
    let _boot = capture_boot_logs();
    tracing::warn!(target: "noisy_lib", "before build_state");

    let seen = Capture::default();
    let config = R2eConfig::from_yaml_str(
        "tracing:\n  console: false\n  ansi: false\n  span-events: none\n  \
         filter: \"warn,noisy_lib=info,r2e_core::runtime::tracing=info\"\n",
    )
    .unwrap();
    let _state = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .tracing_layer(seen.clone())
        .plugin(NoisyLib)
        .build_state()
        .await;

    let options = installed().expect("build_state installed the subscriber");
    assert!(
        !options.console,
        "`tracing.console: false` reached the install"
    );
    assert_eq!(
        options.filter,
        "warn,noisy_lib=info,r2e_core::runtime::tracing=info"
    );

    // The buffered event is replayed first, then the install summary; no
    // "no boot-log capture" WARN.
    let boot = seen.take();
    assert_eq!(boot.len(), 2, "{boot:?}");
    assert_eq!(boot[0], "WARN noisy_lib before build_state");
    assert_eq!(
        boot[1],
        "INFO r2e_core::runtime::tracing r2e: tracing installed: format=pretty \
         filter=\"warn,noisy_lib=info,r2e_core::runtime::tracing=info\" ansi=off \
         span_events=none console=off layers=3 source=config"
    );

    // The plugin's `Suppress` vetoes exactly its (level, target, message);
    // its siblings pass, and the global filter applies to contributed layers.
    tracing::error!(target: "noisy_lib", "harmless noise");
    tracing::error!(target: "noisy_lib", "real problem");
    tracing::info!(target: "noisy_lib", "harmless noise");
    tracing::info!(target: "elsewhere", "filtered out");
    // The plugin's `LogFields` writer decorates events in its spans.
    tracing::info_span!(target: "noisy_lib", "checkout").in_scope(|| {
        tracing::warn!(target: "noisy_lib", "inside");
    });
    assert_eq!(
        seen.take(),
        [
            "ERROR noisy_lib real problem",
            "INFO noisy_lib harmless noise",
            "WARN noisy_lib inside request=checkout",
        ]
    );

    // A second build in the same process keeps the first subscriber; its
    // contributed layers are ignored (one DEBUG line, not an error).
    let late = Capture::default();
    let _again = AppBuilder::new()
        .tracing_layer(late.clone())
        .build_state()
        .await;
    assert_eq!(installed(), Some(options), "the record never moves");
    tracing::error!(target: "noisy_lib", "after the second build");
    assert!(late.take().is_empty(), "the late layer was not added");
    assert_eq!(seen.take(), ["ERROR noisy_lib after the second build"]);
}
