//! Shared fixtures for the `http` target: the thread-local `tracing` capture
//! used by the `HttpTrace` and panic-layer tests.
//!
//! A test installs it with `tracing::subscriber::set_default` on a
//! `current_thread` runtime, so the whole request is polled on the test thread
//! and the capture cannot be interleaved by the other tests of this binary.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

// ── Capture ────────────────────────────────────────────────────────────────

/// One recorded span or event: its identity plus its fields as strings.
#[derive(Clone, Debug)]
pub struct Rec {
    pub name: String,
    pub target: String,
    pub level: Level,
    pub fields: HashMap<String, String>,
}

impl Rec {
    pub fn field(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(String::as_str)
    }

    pub fn mentions(&self, needle: &str) -> bool {
        self.fields.values().any(|v| v.contains(needle))
    }
}

struct FieldRecorder<'a>(&'a mut HashMap<String, String>);

impl Visit for FieldRecorder<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().to_string(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }
}

#[derive(Default, Clone)]
pub struct Capture {
    /// Spans keyed by id, so a late `Span::record` lands on the right one.
    spans: Arc<Mutex<Vec<(u64, Rec)>>>,
    events: Arc<Mutex<Vec<Rec>>>,
}

impl Capture {
    pub fn spans(&self) -> Vec<Rec> {
        self.spans
            .lock()
            .unwrap()
            .iter()
            .map(|(_, rec)| rec.clone())
            .collect()
    }

    pub fn events(&self) -> Vec<Rec> {
        self.events.lock().unwrap().clone()
    }

    /// The single `request` span of the request just driven.
    pub fn request_span(&self) -> Rec {
        let spans = self.spans();
        let mut it = spans.iter().filter(|s| s.name == "request");
        let span = it.next().cloned().expect("a `request` span");
        assert!(it.next().is_none(), "expected exactly one request span");
        span
    }

    /// Summary events (`request completed`) of the request just driven.
    pub fn summaries(&self) -> Vec<Rec> {
        self.events()
            .into_iter()
            .filter(|e| e.field("message") == Some("request completed"))
            .collect()
    }
}

impl<S: Subscriber> Layer<S> for Capture {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, _ctx: Context<'_, S>) {
        let mut fields = HashMap::new();
        attrs.record(&mut FieldRecorder(&mut fields));
        self.spans.lock().unwrap().push((
            id.into_u64(),
            Rec {
                name: attrs.metadata().name().to_string(),
                target: attrs.metadata().target().to_string(),
                level: *attrs.metadata().level(),
                fields,
            },
        ));
    }

    fn on_record(&self, id: &Id, values: &tracing::span::Record<'_>, _ctx: Context<'_, S>) {
        // `Span::record` lands here — `request_id`, `status`, … are recorded
        // after creation, so the capture has to follow them.
        let mut spans = self.spans.lock().unwrap();
        if let Some((_, rec)) = spans.iter_mut().find(|(sid, _)| *sid == id.into_u64()) {
            values.record(&mut FieldRecorder(&mut rec.fields));
        }
    }

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = HashMap::new();
        event.record(&mut FieldRecorder(&mut fields));
        self.events.lock().unwrap().push(Rec {
            name: event.metadata().name().to_string(),
            target: event.metadata().target().to_string(),
            level: *event.metadata().level(),
            fields,
        });
    }
}
