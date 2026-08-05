use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use opentelemetry::trace::TracerProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use r2e_observability::tracing_setup::{normalized_otlp_traces_endpoint, TraceIdFormat};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;

#[derive(Clone, Default)]
struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

struct CaptureGuard(Arc<Mutex<Vec<u8>>>);

impl Write for CaptureGuard {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'writer> MakeWriter<'writer> for CaptureWriter {
    type Writer = CaptureGuard;

    fn make_writer(&'writer self) -> Self::Writer {
        CaptureGuard(self.0.clone())
    }
}

impl CaptureWriter {
    fn output(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

#[test]
fn text_logs_include_trace_and_span_ids() {
    let capture = CaptureWriter::default();
    let provider = SdkTracerProvider::builder().build();
    let tracer = provider.tracer("format-test");
    let event_format = tracing_subscriber::fmt::format().with_ansi(false);
    let fmt_layer = tracing_subscriber::fmt::layer()
        .event_format(TraceIdFormat::text(event_format))
        .with_writer(capture.clone());
    let subscriber = tracing_subscriber::Registry::default()
        .with(fmt_layer)
        .with(tracing_opentelemetry::layer().with_tracer(tracer));
    let subscriber_guard = tracing::subscriber::set_default(subscriber);

    let span = tracing::info_span!("request");
    let span_guard = span.enter();
    tracing::info!("hello");
    drop(span_guard);
    drop(subscriber_guard);

    let output = capture.output();
    let trace_id = output
        .split("trace_id=")
        .nth(1)
        .unwrap()
        .split(' ')
        .next()
        .unwrap();
    let span_id = output
        .split("span_id=")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap();
    assert_eq!(trace_id.len(), 32);
    assert_eq!(span_id.len(), 16);
    provider.shutdown().unwrap();
}

#[test]
fn json_logs_include_trace_and_span_ids() {
    let capture = CaptureWriter::default();
    let provider = SdkTracerProvider::builder().build();
    let tracer = provider.tracer("format-test");
    let event_format = tracing_subscriber::fmt::format().json().with_ansi(false);
    let fmt_layer = tracing_subscriber::fmt::layer()
        .fmt_fields(tracing_subscriber::fmt::format::JsonFields::new())
        .event_format(TraceIdFormat::json(event_format))
        .with_writer(capture.clone());
    let subscriber = tracing_subscriber::Registry::default()
        .with(fmt_layer)
        .with(tracing_opentelemetry::layer().with_tracer(tracer));
    let subscriber_guard = tracing::subscriber::set_default(subscriber);

    let span = tracing::info_span!("request");
    let span_guard = span.enter();
    tracing::info!("hello");
    drop(span_guard);
    drop(subscriber_guard);

    let value: serde_json::Value = serde_json::from_str(capture.output().trim()).unwrap();
    assert_eq!(value["trace_id"].as_str().unwrap().len(), 32);
    assert_eq!(value["span_id"].as_str().unwrap().len(), 16);
    provider.shutdown().unwrap();
}

#[test]
fn formatter_adds_no_noise_without_opentelemetry() {
    let capture = CaptureWriter::default();
    let event_format = tracing_subscriber::fmt::format().with_ansi(false);
    let fmt_layer = tracing_subscriber::fmt::layer()
        .event_format(TraceIdFormat::text(event_format))
        .with_writer(capture.clone());
    let subscriber = tracing_subscriber::Registry::default().with(fmt_layer);
    let subscriber_guard = tracing::subscriber::set_default(subscriber);

    let span = tracing::info_span!("request");
    let span_guard = span.enter();
    tracing::info!("hello");
    drop(span_guard);
    drop(subscriber_guard);

    let output = capture.output();
    assert!(!output.contains("trace_id="));
    assert!(!output.contains("span_id="));
}

#[test]
fn normalizes_only_pathless_http_endpoints() {
    assert_eq!(
        normalized_otlp_traces_endpoint("http://localhost:4318"),
        "http://localhost:4318/v1/traces"
    );
    assert_eq!(
        normalized_otlp_traces_endpoint("https://collector.example/"),
        "https://collector.example/v1/traces"
    );
    assert_eq!(
        normalized_otlp_traces_endpoint("http://collector:4318/custom/traces"),
        "http://collector:4318/custom/traces"
    );
    assert_eq!(
        normalized_otlp_traces_endpoint("unix:///tmp/otel.sock"),
        "unix:///tmp/otel.sock"
    );
}
