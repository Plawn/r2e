use std::io::IsTerminal;

use tracing_subscriber::fmt::{self, MakeWriter};
use tracing_subscriber::layer::Layer;

use crate::enrich::Enriched;
use crate::late::BoxedLayer;
use crate::options::{LogFormat, TracingOptions};

/// The console layer [`try_install`](crate::try_install) puts in the stack,
/// writing to stdout.
pub(crate) fn console_layer(opts: &TracingOptions) -> BoxedLayer {
    console_layer_with_writer(opts, std::io::stdout)
}

/// The console layer, writing to `make_writer` — `fmt` (JSON under
/// [`LogFormat::Json`]) with the knobs of `opts` and the registered
/// [enrichers](crate::add_enricher).
///
/// Exposed so tests can capture the exact lines the installed subscriber
/// writes without installing the process-global subscriber.
#[doc(hidden)]
pub fn console_layer_with_writer<W>(opts: &TracingOptions, make_writer: W) -> BoxedLayer
where
    W: for<'writer> MakeWriter<'writer> + Send + Sync + 'static,
{
    // Unset → colour only on a terminal: piped/collected logs stay clean.
    let ansi = opts.ansi.unwrap_or_else(|| std::io::stdout().is_terminal());
    // `ansi` is set on the format too: the enrichment path renders into a
    // plain buffer, whose `Writer` would otherwise report "no colours".
    let format = fmt::format()
        .with_target(opts.target.unwrap_or(true))
        .with_thread_ids(opts.thread_ids.unwrap_or(false))
        .with_thread_names(opts.thread_names.unwrap_or(false))
        .with_file(opts.file.unwrap_or(false))
        .with_line_number(opts.line_number.unwrap_or(false))
        .with_level(opts.level.unwrap_or(true))
        .with_ansi(ansi);
    let layer = fmt::layer()
        .with_writer(make_writer)
        .with_ansi(ansi)
        .with_span_events(opts.effective_span_events());
    match opts.effective_format() {
        LogFormat::Json => layer
            .fmt_fields(fmt::format::JsonFields::new())
            .event_format(Enriched::json(format.json()))
            .boxed(),
        LogFormat::Pretty => layer.event_format(Enriched::text(format)).boxed(),
    }
}
