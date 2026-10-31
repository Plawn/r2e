use std::fmt::{self, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use tracing::{span, Event, Metadata, Subscriber};
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::fmt::FmtContext;
use tracing_subscriber::registry::LookupSpan;

/// What a [`FieldEnricher`] knows about the event being written.
#[non_exhaustive]
#[derive(Debug)]
pub struct EnrichContext<'a> {
    /// The event's metadata (level, target, module, file/line).
    pub metadata: &'static Metadata<'static>,
    /// The event itself, for enrichers that read its fields.
    pub event: &'a Event<'a>,
    /// Whether the event has a parent span (explicit, or the current span).
    pub has_parent_span: bool,
    /// The id of that parent span, when there is one.
    pub span_id: Option<span::Id>,
}

/// Adds fields to every line the console layer writes.
///
/// Called once per event, on the thread that emits it, while the line is
/// formatted; `out(key, value)` appends a field — ` key=value` in the text
/// format, `"key":"value"` (a JSON string) in the JSON format. Emitting
/// nothing leaves the line untouched. Keep it cheap and do not log from it.
///
/// ```rust,ignore
/// struct Region;
/// impl r2e_tracing::FieldEnricher for Region {
///     fn enrich(
///         &self,
///         _cx: &r2e_tracing::EnrichContext<'_>,
///         out: &mut dyn FnMut(&'static str, &dyn std::fmt::Display),
///     ) {
///         out("region", &"eu-west-1");
///     }
/// }
/// r2e_tracing::add_enricher(Box::new(Region));
/// ```
pub trait FieldEnricher: Send + Sync + 'static {
    /// Append this enricher's fields for the event described by `cx`.
    fn enrich(&self, cx: &EnrichContext<'_>, out: &mut dyn FnMut(&'static str, &dyn fmt::Display));
}

/// Copy-on-write list: formatting takes a snapshot and releases the lock
/// before calling enrichers, so an enricher that (wrongly) logs cannot
/// deadlock against a concurrent [`add_enricher`].
type Enrichers = Arc<Vec<Arc<dyn FieldEnricher>>>;

static ENRICHERS: RwLock<Option<Enrichers>> = RwLock::new(None);

/// `true` once an enricher exists: with none, the console format is a
/// pass-through costing one relaxed load per event.
static ANY_ENRICHER: AtomicBool = AtomicBool::new(false);

/// Register a [`FieldEnricher`] on the console layer.
///
/// Process-wide and append-only; it may be called before or after the
/// subscriber is installed, and applies to every event formatted from then on.
pub fn add_enricher(enricher: Box<dyn FieldEnricher>) {
    let mut slot = ENRICHERS.write().unwrap_or_else(PoisonError::into_inner);
    let mut list = slot.as_deref().cloned().unwrap_or_default();
    list.push(Arc::from(enricher));
    *slot = Some(Arc::new(list));
    ANY_ENRICHER.store(true, Ordering::Release);
}

pub(crate) fn snapshot() -> Option<Enrichers> {
    if !ANY_ENRICHER.load(Ordering::Relaxed) {
        return None;
    }
    ENRICHERS
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// Wraps the console layer's event format and splices the registered
/// enrichers' fields into each rendered line.
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
        let Some(enrichers) = snapshot() else {
            return self.inner.format_event(ctx, writer, event);
        };

        let span_id = ctx.parent_span().map(|span| span.id());
        let cx = EnrichContext {
            metadata: event.metadata(),
            event,
            has_parent_span: span_id.is_some(),
            span_id,
        };
        let mut extra = String::new();
        let json = self.json;
        for enricher in enrichers.iter() {
            enricher.enrich(&cx, &mut |key, value| {
                push_field(&mut extra, json, key, value)
            });
        }
        if extra.is_empty() {
            return self.inner.format_event(ctx, writer, event);
        }

        // The console format carries its own `ansi` setting, so rendering
        // into a plain buffer keeps the colours.
        let mut rendered = String::new();
        self.inner
            .format_event(ctx, Writer::new(&mut rendered), event)?;
        let at = if json {
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
