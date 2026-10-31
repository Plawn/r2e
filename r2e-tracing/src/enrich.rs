use std::fmt::{self, Write as _};

use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::fmt::FmtContext;
use tracing_subscriber::registry::LookupSpan;

/// Extra fields the console prints on every line emitted inside a span.
///
/// A span **extension**: a contributed layer inserts it in `on_new_span` (or
/// later, through `extensions_mut().get_mut::<LogFields>()`), and the console
/// layer of [`install`](crate::install) appends its fields to every event in
/// that span's scope — ` key=value` in text, `"key":"value"` (a JSON string)
/// in JSON, a top-level key in flat JSON (where the formatter's reserved keys
/// still win). When several spans of the scope carry the same key, the
/// **closest** span wins.
///
/// ```rust,ignore
/// impl<S> Layer<S> for TraceIds where S: Subscriber + for<'a> LookupSpan<'a> {
///     fn on_new_span(&self, _: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
///         let span = ctx.span(id).unwrap();
///         let mut fields = LogFields::new();
///         fields.insert("trace_id", current_trace_id());
///         span.extensions_mut().insert(fields);
///     }
/// }
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogFields {
    fields: Vec<(&'static str, String)>,
}

impl LogFields {
    /// No field.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set `key` to `value`: replaces the value in place when `key` is already
    /// set, appends it otherwise (fields keep their insertion order).
    pub fn insert(&mut self, key: &'static str, value: impl fmt::Display) {
        let value = value.to_string();
        match self.fields.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => *v = value,
            None => self.fields.push((key, value)),
        }
    }

    /// The fields, in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.fields.iter().map(|(k, v)| (*k, v.as_str()))
    }

    /// Whether no field is set.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

/// The [`LogFields`] of the event's span scope, merged: leaf to root, the
/// closest span winning a key. Empty outside any span.
pub(crate) fn collect<S, N>(ctx: &FmtContext<'_, S, N>) -> Vec<(&'static str, String)>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    N: for<'writer> FormatFields<'writer> + 'static,
{
    let mut out: Vec<(&'static str, String)> = Vec::new();
    let Some(scope) = ctx.event_scope() else {
        return out;
    };
    for span in scope {
        let extensions = span.extensions();
        let Some(fields) = extensions.get::<LogFields>() else {
            continue;
        };
        for (key, value) in &fields.fields {
            if !out.iter().any(|(k, _)| k == key) {
                out.push((key, value.clone()));
            }
        }
    }
    out
}

/// Wraps the console layer's event format and splices the scope's
/// [`LogFields`] into each rendered line.
pub(crate) struct Enriched<F> {
    inner: F,
    json: bool,
}

impl<F> Enriched<F> {
    pub(crate) fn text(inner: F) -> Self {
        Self { inner, json: false }
    }

    pub(crate) fn json(inner: F) -> Self {
        Self { inner, json: true }
    }
}

impl<S, N, F> FormatEvent<S, N> for Enriched<F>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    N: for<'writer> FormatFields<'writer> + 'static,
    F: FormatEvent<S, N>,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let fields = collect(ctx);
        let mut extra = String::new();
        for (key, value) in &fields {
            push_field(&mut extra, self.json, key, value);
        }
        if extra.is_empty() {
            return self.inner.format_event(ctx, writer, event);
        }

        // The console format carries its own `ansi` setting, so rendering
        // into a plain buffer keeps the colours.
        let mut rendered = String::new();
        self.inner
            .format_event(ctx, Writer::new(&mut rendered), event)?;
        let at = if self.json {
            rendered.rfind('}').unwrap_or(rendered.len())
        } else if rendered.ends_with('\n') {
            rendered.len() - 1
        } else {
            rendered.len()
        };
        writer.write_str(&rendered[..at])?;
        writer.write_str(&extra)?;
        writer.write_str(&rendered[at..])
    }
}

fn push_field(out: &mut String, json: bool, key: &str, value: &dyn fmt::Display) {
    if json {
        out.push_str(",\"");
        push_json_escaped(out, key);
        out.push_str("\":\"");
        push_json_escaped(out, &value.to_string());
        out.push('"');
    } else {
        let _ = write!(out, " {key}={value}");
    }
}

fn push_json_escaped(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
}
