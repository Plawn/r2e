//! The console layer's line formats: flat JSON (`tracing.json.flatten:
//! true`), and the `LogFields` span extension printed in text, JSON and flat
//! JSON.
//!
//! The console layer writes into a buffer under a scoped subscriber; the
//! global subscriber is never installed.

use std::io;
use std::sync::{Arc, Mutex};

use r2e_tracing::{
    console_layer_with_writer, BoxedLayer, JsonOptions, LogFields, LogFormat, SpanEvents,
    TracingOptions,
};
use serde_json::Value;
use tracing::span::{Attributes, Id};
use tracing::Subscriber;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
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

/// Flat JSON options, with `json` tweaked by the caller.
fn flat(json: JsonOptions) -> TracingOptions {
    TracingOptions::new("trace")
        .with_format(LogFormat::Json)
        .with_span_events(SpanEvents::None)
        .with_json(json.with_flatten(true))
}

/// Everything written through a console layer configured by `opts`, after
/// the `before` layers (the way `install` orders contributed layers).
fn raw(opts: &TracingOptions, before: Vec<BoxedLayer>, emit: impl FnOnce()) -> String {
    let buffer = Buffer::default();
    let mut layers = before;
    layers.push(console_layer_with_writer(opts, buffer.clone()));
    let subscriber = Registry::default().with(layers);
    tracing::subscriber::with_default(subscriber, emit);
    buffer.take()
}

/// Every line written through a console layer configured by `opts`, parsed.
fn lines(opts: &TracingOptions, emit: impl FnOnce()) -> Vec<Value> {
    parse(raw(opts, vec![], emit))
}

fn parse(out: String) -> Vec<Value> {
    assert!(out.ends_with('\n'), "{out:?}");
    out.lines()
        .map(|line| serde_json::from_str(line).expect("one JSON object per line"))
        .collect()
}

fn one(opts: &TracingOptions, emit: impl FnOnce()) -> Value {
    let mut all = lines(opts, emit);
    assert_eq!(all.len(), 1, "{all:?}");
    all.remove(0)
}

#[test]
fn message_and_fields_at_top_level() {
    let line = one(
        &flat(JsonOptions::default()),
        || tracing::warn!(target: "flat_test", pairs = 3, ok = true, who = "me", "slow op"),
    );
    assert_eq!(line["message"], "slow op");
    assert_eq!(line["pairs"], 3);
    assert_eq!(line["ok"], true);
    assert_eq!(line["who"], "me");
    assert_eq!(line["level"], "WARN");
    assert_eq!(line["target"], "flat_test");
    assert!(line.get("fields").is_none(), "{line}");
    let timestamp = line["timestamp"].as_str().expect("timestamp string");
    assert!(
        timestamp.len() >= 20 && timestamp.ends_with('Z') && timestamp.as_bytes()[10] == b'T',
        "{timestamp}"
    );
}

#[test]
fn custom_message_key() {
    let line = one(
        &flat(JsonOptions::default().with_message_key("_msg")),
        || tracing::info!(target: "flat_test", "hello"),
    );
    assert_eq!(line["_msg"], "hello");
    assert!(line.get("message").is_none(), "{line}");
}

#[test]
fn target_stands_in_for_a_missing_message() {
    let line = one(
        &flat(JsonOptions::default().with_message_key("_msg")),
        || tracing::info!(target: "flat_test", count = 1),
    );
    assert_eq!(line["_msg"], "flat_test");
    assert_eq!(line["count"], 1);
}

#[test]
fn span_and_span_list_root_first() {
    let line = one(&flat(JsonOptions::default()), || {
        let _outer = tracing::info_span!("run", run_id = 7, pipeline = "v6").entered();
        let inner = tracing::info_span!("step", n = tracing::field::Empty).entered();
        inner.record("n", 2);
        tracing::info!(target: "flat_test", "inside");
    });
    assert_eq!(line["span"]["name"], "step");
    assert_eq!(line["span"]["n"], 2);
    assert_eq!(line["spans"][0]["name"], "run");
    assert_eq!(line["spans"][0]["run_id"], 7);
    assert_eq!(line["spans"][0]["pipeline"], "v6");
    assert_eq!(line["spans"][1]["name"], "step");
    assert_eq!(line["spans"].as_array().unwrap().len(), 2);
}

#[test]
fn span_keys_follow_the_options() {
    let emit = || {
        let _outer = tracing::info_span!("run").entered();
        tracing::info!(target: "flat_test", "inside");
    };
    let no_list = one(&flat(JsonOptions::default().with_span_list(false)), emit);
    assert_eq!(no_list["span"]["name"], "run");
    assert!(no_list.get("spans").is_none(), "{no_list}");

    let neither = one(
        &flat(
            JsonOptions::default()
                .with_span_list(false)
                .with_current_span(false),
        ),
        emit,
    );
    assert!(neither.get("span").is_none(), "{neither}");
    assert!(neither.get("spans").is_none(), "{neither}");

    // Outside any span: no span keys at all.
    let outside = one(
        &flat(JsonOptions::default()),
        || tracing::info!(target: "flat_test", "outside"),
    );
    assert!(outside.get("span").is_none(), "{outside}");
    assert!(outside.get("spans").is_none(), "{outside}");
}

#[test]
fn reserved_keys_win_over_event_fields() {
    let line = one(
        &flat(JsonOptions::default().with_message_key("_msg")),
        || {
            tracing::info!(
                target: "flat_test",
                level = "spoofed",
                target = "spoofed",
                timestamp = 0,
                _msg = "spoofed",
                "real"
            )
        },
    );
    assert_eq!(line["level"], "INFO");
    assert_eq!(line["target"], "flat_test");
    assert!(line["timestamp"].is_string(), "{line}");
    assert_eq!(line["_msg"], "real");
}

#[test]
fn optional_knobs_add_their_keys() {
    let opts = flat(JsonOptions::default())
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_level(false)
        .with_target(false);
    let line = one(&opts, || tracing::info!(target: "flat_test", "knobs"));
    assert!(
        line["file"].as_str().unwrap().ends_with("main.rs"),
        "{line}"
    );
    assert!(line["line"].is_u64(), "{line}");
    assert!(
        line["thread_id"].as_str().unwrap().starts_with("ThreadId("),
        "{line}"
    );
    assert!(line.get("level").is_none(), "{line}");
    assert!(line.get("target").is_none(), "{line}");
}

#[test]
fn log_bridged_events_are_normalized() {
    let line = one(&flat(JsonOptions::default()), || {
        let record = log::Record::builder()
            .args(format_args!("from log"))
            .level(log::Level::Warn)
            .target("legacy::module")
            .module_path_static(Some("legacy::module"))
            .file_static(Some("legacy.rs"))
            .line(Some(12))
            .build();
        tracing_log::format_trace(&record).unwrap();
    });
    assert_eq!(line["message"], "from log");
    assert_eq!(line["level"], "WARN");
    assert_eq!(line["target"], "legacy::module");
    let object = line.as_object().unwrap();
    assert!(object.keys().all(|key| !key.starts_with("log.")), "{line}");
}

/// Inserts `LogFields` on every new span: `span_name=<name>` always,
/// `trace_id=<the span's "trace" field>` when it has one — what an OTel ids
/// layer does.
struct Ids;

impl<S> Layer<S> for Ids
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        struct Trace(Option<String>);
        impl tracing::field::Visit for Trace {
            fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
                if f.name() == "trace" {
                    self.0 = Some(format!("{v:?}"));
                }
            }
        }
        let mut trace = Trace(None);
        attrs.record(&mut trace);
        let mut fields = LogFields::new();
        fields.insert("span_name", attrs.metadata().name());
        if let Some(t) = trace.0 {
            fields.insert("trace_id", t);
        }
        ctx.span(id).unwrap().extensions_mut().insert(fields);
    }
}

/// Emits one event in `inner` (trace 22) nested in `outer` (trace 11).
fn nested() {
    let _outer = tracing::info_span!("outer", trace = 11).entered();
    let _inner = tracing::info_span!("inner", trace = 22).entered();
    tracing::info!(target: "fields_test", "hello");
}

fn plain_text() -> TracingOptions {
    TracingOptions::new("trace")
        .with_ansi(false)
        .with_span_events(SpanEvents::None)
}

#[test]
fn log_fields_are_spliced_into_text_lines() {
    // Outside any span: the plain line.
    let plain = raw(
        &plain_text(),
        vec![Box::new(Ids)],
        || tracing::info!(target: "fields_test", "hello"),
    );
    assert!(plain.ends_with("hello\n"), "{plain:?}");

    // Closest span wins a key; insertion order otherwise.
    let line = raw(&plain_text(), vec![Box::new(Ids)], nested);
    assert!(
        line.ends_with("hello span_name=inner trace_id=22\n"),
        "{line:?}"
    );
    assert_eq!(line.matches('\n').count(), 1);
}

#[test]
fn log_fields_are_members_of_json_lines() {
    let opts = plain_text().with_format(LogFormat::Json);
    let out = raw(&opts, vec![Box::new(Ids)], nested);
    assert!(out.ends_with("}\n"), "{out:?}");
    let line: Value = serde_json::from_str(out.trim_end()).expect("valid JSON");
    assert_eq!(line["trace_id"], "22");
    assert_eq!(line["span_name"], "inner");
    assert_eq!(line["fields"]["message"], "hello");
}

#[test]
fn log_fields_land_in_the_flat_object() {
    let out = raw(&flat(JsonOptions::default()), vec![Box::new(Ids)], || {
        let _s = tracing::info_span!("outer", trace = 11).entered();
        tracing::info!(target: "fields_test", trace_id = "event", "hello");
    });
    let mut all = parse(out);
    assert_eq!(all.len(), 1, "{all:?}");
    let line = all.remove(0);
    // `LogFields` win over event fields, lose to reserved keys.
    assert_eq!(line["trace_id"], "11");
    assert_eq!(line["span_name"], "outer");
    assert_eq!(line["message"], "hello");
}

#[test]
fn log_fields_insert_replaces_in_place() {
    let mut fields = LogFields::new();
    assert!(fields.is_empty());
    fields.insert("a", 1);
    fields.insert("b", "two");
    fields.insert("a", 3);
    assert_eq!(
        fields.iter().collect::<Vec<_>>(),
        [("a", "3"), ("b", "two")]
    );
}

#[test]
fn nested_json_is_unchanged_without_flatten() {
    let opts = TracingOptions::new("trace")
        .with_format(LogFormat::Json)
        .with_span_events(SpanEvents::None);
    assert!(!opts.json.flatten);
    let line = one(
        &opts,
        || tracing::info!(target: "flat_test", n = 1, "nested"),
    );
    assert_eq!(line["fields"]["message"], "nested");
    assert_eq!(line["fields"]["n"], 1);
    assert!(line.get("n").is_none(), "{line}");
}
