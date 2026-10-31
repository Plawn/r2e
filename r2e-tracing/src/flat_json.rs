//! The flat JSON line format (`tracing.json.flatten: true`).
//!
//! One JSON object per event, every key at the top level:
//!
//! ```text
//! {"message":"listening","addr":"0.0.0.0:3000","level":"INFO","target":"r2e",
//!  "timestamp":"2026-10-31T09:12:03.123456Z","span":{"name":"boot"},"spans":[{"name":"boot"}]}
//! ```
//!
//! Unlike `tracing-subscriber`'s JSON format (message and fields nested under
//! `"fields"`), this is the shape log stores such as VictoriaLogs and Loki
//! index directly: VictoriaLogs only needs `message_key` set to `_msg`.

use std::fmt;

use serde_json::{Map, Value};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Subscriber};
use tracing_log::NormalizeEvent;
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::fmt::time::{FormatTime, SystemTime};
use tracing_subscriber::fmt::FmtContext;
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

use crate::enrich;
use crate::options::{JsonOptions, TracingOptions};

/// A span's fields as a JSON map, kept in the span's extensions by
/// [`SpanFieldsLayer`] for [`FlatJson`] to read.
struct SpanFields(Map<String, Value>);

/// Records each span's fields as a [`SpanFields`] map — built from the field
/// visitor directly, so no formatted string is ever parsed back.
pub(crate) struct SpanFieldsLayer;

impl<S> Layer<S> for SpanFieldsLayer
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let mut fields = Map::new();
        attrs.record(&mut FieldVisitor(&mut fields));
        span.extensions_mut().insert(SpanFields(fields));
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let mut extensions = span.extensions_mut();
        if let Some(SpanFields(fields)) = extensions.get_mut::<SpanFields>() {
            values.record(&mut FieldVisitor(fields));
        }
    }
}

/// The flat JSON event format.
///
/// Key precedence, lowest first: event fields, then
/// [`LogFields`](crate::LogFields) of the span scope, then the formatter's own keys —
/// the **reserved keys** `<message_key>`, `timestamp`, `level`, `target`,
/// `span`, `spans`, `thread_id`, `thread_name`, `file` and `line`. A reserved
/// key overwrites an event or `LogFields` field of the same name whenever the
/// formatter writes it (`level`, `target`, `thread_*`, `file`, `line` follow
/// the matching [`TracingOptions`] knobs; `span`/`spans` are written when the
/// event sits in a span and [`JsonOptions`] asks for them).
///
/// Events bridged from the `log` crate are reported with their real level and
/// target, and their `log.*` bookkeeping fields are dropped.
pub(crate) struct FlatJson {
    json: JsonOptions,
    level: bool,
    target: bool,
    thread_ids: bool,
    thread_names: bool,
    file: bool,
    line_number: bool,
}

impl FlatJson {
    pub(crate) fn new(opts: &TracingOptions) -> Self {
        Self {
            json: opts.json.clone(),
            level: opts.level.unwrap_or(true),
            target: opts.target.unwrap_or(true),
            thread_ids: opts.thread_ids.unwrap_or(false),
            thread_names: opts.thread_names.unwrap_or(false),
            file: opts.file.unwrap_or(false),
            line_number: opts.line_number.unwrap_or(false),
        }
    }
}

impl<S, N> FormatEvent<S, N> for FlatJson
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    N: for<'writer> FormatFields<'writer> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        // Events bridged from the `log` crate carry their real level/target
        // in `log.*` fields (which the visitor drops).
        let normalized = event.normalized_metadata();
        let meta = normalized.as_ref().unwrap_or_else(|| event.metadata());

        let mut line = Map::new();
        event.record(&mut FieldVisitor(&mut line));
        let message = line.remove("message");

        for (key, value) in enrich::collect(ctx) {
            line.insert(key.to_owned(), Value::String(value));
        }

        // Reserved keys last: they win over any colliding field.
        line.insert(
            self.json.message_key.clone(),
            message.unwrap_or_else(|| meta.target().into()),
        );
        let mut timestamp = String::new();
        SystemTime.format_time(&mut Writer::new(&mut timestamp))?;
        line.insert("timestamp".into(), timestamp.into());
        if self.level {
            line.insert("level".into(), meta.level().as_str().into());
        }
        if self.target {
            line.insert("target".into(), meta.target().into());
        }
        if self.thread_ids {
            let id = format!("{:?}", std::thread::current().id());
            line.insert("thread_id".into(), id.into());
        }
        if self.thread_names
            && let Some(name) = std::thread::current().name()
        {
            line.insert("thread_name".into(), name.into());
        }
        if self.file
            && let Some(file) = meta.file()
        {
            line.insert("file".into(), file.into());
        }
        if self.line_number
            && let Some(number) = meta.line()
        {
            line.insert("line".into(), number.into());
        }
        self.insert_spans(ctx, &mut line);

        writer.write_str(&Value::Object(line).to_string())?;
        writer.write_char('\n')
    }
}

impl FlatJson {
    fn insert_spans<S, N>(&self, ctx: &FmtContext<'_, S, N>, line: &mut Map<String, Value>)
    where
        S: Subscriber + for<'lookup> LookupSpan<'lookup>,
        N: for<'writer> FormatFields<'writer> + 'static,
    {
        if !self.json.current_span && !self.json.span_list {
            return;
        }
        let Some(scope) = ctx.event_scope() else {
            return;
        };
        // `scope` runs leaf first; `spans` is written root first.
        let mut spans: Vec<Value> = if self.json.span_list {
            scope.map(|span| span_object(&span)).collect()
        } else {
            scope.take(1).map(|span| span_object(&span)).collect()
        };
        if self.json.current_span
            && let Some(current) = spans.first()
        {
            line.insert("span".into(), current.clone());
        }
        if self.json.span_list && !spans.is_empty() {
            spans.reverse();
            line.insert("spans".into(), Value::Array(spans));
        }
    }
}

/// `{"name": <span name>, <span fields>}` — the name wins over a field
/// called `name`.
fn span_object<S>(span: &tracing_subscriber::registry::SpanRef<'_, S>) -> Value
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    let mut object = span
        .extensions()
        .get::<SpanFields>()
        .map(|fields| fields.0.clone())
        .unwrap_or_default();
    object.insert("name".into(), span.name().into());
    Value::Object(object)
}

/// Records fields into a JSON map with their native JSON types; `log.*`
/// fields (the `log`-crate bridge's bookkeeping) are dropped.
struct FieldVisitor<'a>(&'a mut Map<String, Value>);

impl FieldVisitor<'_> {
    fn insert(&mut self, field: &Field, value: Value) {
        let name = field.name();
        if !name.starts_with("log.") {
            self.0.insert(name.to_owned(), value);
        }
    }
}

impl Visit for FieldVisitor<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.insert(field, value.into());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.insert(field, value.into());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.insert(field, value.into());
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.insert(field, value.into());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.insert(field, value.into());
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.insert(field, value.to_string().into());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.insert(field, format!("{value:?}").into());
    }
}
