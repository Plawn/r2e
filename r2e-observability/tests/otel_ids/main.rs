//! The `trace_id`/`span_id` [`LogFields`](r2e_core::LogFields) layer
//! `Observability` contributes, on R2E's real console layer.
//!
//! The console layer writes into a buffer under a **scoped** subscriber
//! (`with_default`) next to a `tracing-opentelemetry` layer, so the lines can
//! be read back — the global subscriber `otel_contrib` exercises writes to
//! stdout and cannot be captured. The contribution through
//! `PluginSetupContext::tracing_layer` is covered there.

use std::io;
use std::sync::{Arc, Mutex};

use opentelemetry::trace::TracerProvider;
use opentelemetry_sdk::trace::InMemorySpanExporter;
use opentelemetry_sdk::trace::SdkTracerProvider;
use r2e_core::r2e_tracing::console_layer_with_writer;
use r2e_core::{LogFormat, TracingOptions};
use r2e_observability::tracing_setup::OtelIds;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Registry;

#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Buffer {
    fn take(&self) -> String {
        String::from_utf8(std::mem::take(&mut *self.0.lock().unwrap())).unwrap()
    }
}

impl io::Write for Buffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Buffer {
    type Writer = Buffer;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Run `emit` under console layer (`opts`) + OpenTelemetry layer; return the
/// written lines and the exported spans' `(trace_id, span_id)`.
fn run(opts: &TracingOptions, emit: impl FnOnce()) -> (String, Vec<(String, String)>) {
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let buffer = Buffer::default();
    let subscriber = Registry::default()
        .with(console_layer_with_writer(opts, buffer.clone()))
        // The OpenTelemetry layer first, as the plugin contributes them:
        // `OtelIds` reads the context it activates on span enter.
        .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("otel-ids")))
        .with(OtelIds);
    tracing::subscriber::with_default(subscriber, emit);
    let spans = exporter
        .get_finished_spans()
        .unwrap()
        .into_iter()
        .map(|s| {
            (
                s.span_context.trace_id().to_string(),
                s.span_context.span_id().to_string(),
            )
        })
        .collect();
    (buffer.take(), spans)
}

fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split_whitespace()
        .find_map(|token| token.strip_prefix(key)?.strip_prefix('='))
}

#[test]
fn text_lines_inside_a_span_carry_the_exported_ids() {
    let opts = TracingOptions::default().with_ansi(false);
    let (out, spans) = run(&opts, || {
        let span = tracing::info_span!("request");
        let _entered = span.enter();
        tracing::info!("hello");
    });

    let line = out.lines().find(|l| l.contains("hello")).unwrap();
    let ids = (
        field(line, "trace_id").expect("trace_id").to_string(),
        field(line, "span_id").expect("span_id").to_string(),
    );
    assert_eq!(ids.0.len(), 32, "{line}");
    assert_eq!(ids.1.len(), 16, "{line}");
    assert_eq!(
        spans,
        vec![ids],
        "the line names the span that was exported"
    );
}

#[test]
fn json_lines_inside_a_span_carry_the_ids() {
    let opts = TracingOptions::default()
        .with_ansi(false)
        .with_format(LogFormat::Json);
    let (out, _) = run(&opts, || {
        let span = tracing::info_span!("request");
        let _entered = span.enter();
        tracing::info!("hello");
    });

    // JSON closes spans by default: the second line is the span's `close`.
    let line = out.lines().find(|l| l.contains("hello")).unwrap();
    let value: serde_json::Value = r2e_core::json::from_str(line).unwrap();
    assert_eq!(value["trace_id"].as_str().unwrap().len(), 32, "{out}");
    assert_eq!(value["span_id"].as_str().unwrap().len(), 16, "{out}");
}

#[test]
fn lines_outside_a_span_are_left_alone() {
    let opts = TracingOptions::default().with_ansi(false);
    let (out, _) = run(&opts, || tracing::info!("no span"));

    assert!(out.contains("no span"), "{out}");
    assert!(!out.contains("trace_id="), "{out}");
    assert!(!out.contains("span_id="), "{out}");
}
