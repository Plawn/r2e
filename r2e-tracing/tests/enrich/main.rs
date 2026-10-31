//! Enrichers: fields spliced into the console layer's lines, text and JSON.
//!
//! Its **own target**: enrichers are process-wide and append-only, so the
//! "no enricher" baseline and the enriched lines are one ordered sequence.
//! The console layer writes into a buffer under a scoped subscriber; the
//! global subscriber is never installed.

use std::fmt;
use std::io;
use std::sync::{Arc, Mutex};

use r2e_tracing::{
    add_enricher, console_layer_with_writer, EnrichContext, FieldEnricher, LogFormat, SpanEvents,
    TracingOptions,
};
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

/// Writes one line through a console layer configured by `opts`.
fn line(opts: &TracingOptions, emit: impl FnOnce()) -> String {
    let buffer = Buffer::default();
    let subscriber = Registry::default().with(console_layer_with_writer(opts, buffer.clone()));
    tracing::subscriber::with_default(subscriber, emit);
    buffer.take()
}

struct Region;

impl FieldEnricher for Region {
    fn enrich(
        &self,
        _cx: &EnrichContext<'_>,
        out: &mut dyn FnMut(&'static str, &dyn fmt::Display),
    ) {
        out("region", &"eu-west-1");
    }
}

/// Reports whether the event sits in a span, and needs JSON escaping.
struct InSpan;

impl FieldEnricher for InSpan {
    fn enrich(&self, cx: &EnrichContext<'_>, out: &mut dyn FnMut(&'static str, &dyn fmt::Display)) {
        if cx.has_parent_span {
            assert!(cx.span_id.is_some());
            out("in_span", &"\"yes\"");
        }
        assert_eq!(cx.metadata.target(), "enrich_test");
    }
}

#[test]
fn enrichers_append_fields_to_text_and_json_lines() {
    let text = TracingOptions::new("trace")
        .with_ansi(false)
        .with_span_events(SpanEvents::None);
    let json = text.clone().with_format(LogFormat::Json);

    // No enricher: the plain format.
    let plain = line(&text, || tracing::info!(target: "enrich_test", "hello"));
    assert!(plain.ends_with("hello\n"), "{plain:?}");

    add_enricher(Box::new(Region));
    add_enricher(Box::new(InSpan));

    // Text: ` key=value` before the newline.
    let enriched = line(&text, || tracing::info!(target: "enrich_test", "hello"));
    assert!(
        enriched.ends_with("hello region=eu-west-1\n"),
        "{enriched:?}"
    );
    assert_eq!(enriched.matches('\n').count(), 1);

    let in_span = line(&text, || {
        let _s = tracing::info_span!(target: "enrich_test", "outer").entered();
        tracing::info!(target: "enrich_test", "hello");
    });
    assert!(
        in_span.ends_with("hello region=eu-west-1 in_span=\"yes\"\n"),
        "{in_span:?}"
    );

    // JSON: string members before the final `}`, still one valid object.
    let out = line(&json, || {
        let _s = tracing::info_span!(target: "enrich_test", "outer").entered();
        tracing::info!(target: "enrich_test", "hello");
    });
    assert!(out.ends_with("}\n"), "{out:?}");
    let value: serde_json::Value = serde_json::from_str(out.trim_end()).expect("valid JSON");
    assert_eq!(value["region"], "eu-west-1");
    assert_eq!(value["in_span"], "\"yes\"");
    assert_eq!(value["fields"]["message"], "hello");
}
