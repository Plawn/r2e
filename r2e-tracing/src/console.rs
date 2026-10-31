use std::io::IsTerminal;
use std::sync::{Mutex, MutexGuard, PoisonError};

use tracing_subscriber::fmt::{self, MakeWriter};
use tracing_subscriber::layer::Layer;

use crate::enrich::Enriched;
use crate::flat_json::{FlatJson, SpanFieldsLayer};
use crate::install::{installed, AlreadyInstalled};
use crate::late::BoxedLayer;
use crate::options::{LogFormat, TracingOptions};

/// Builds the **whole** console layer of the subscriber
/// [`try_install`](crate::try_install) installs — see [`set_console_factory`].
pub type ConsoleFactory = fn(&TracingOptions) -> BoxedLayer;

/// The registered [`ConsoleFactory`]; `None` = the built-in console layer.
///
/// [`try_install`](crate::try_install) holds this lock for the whole install,
/// so a [`set_console_factory`] runs either entirely before the install (and
/// is used by it) or entirely after (and sees it).
static FACTORY: Mutex<Option<ConsoleFactory>> = Mutex::new(None);

pub(crate) fn lock_factory() -> MutexGuard<'static, Option<ConsoleFactory>> {
    FACTORY.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Replace the console layer of the subscriber R2E installs.
///
/// [`try_install`](crate::try_install) calls `factory(opts)` instead of
/// building its own `fmt` layer; the rest of the stack stays — the
/// [late-layer slot](crate::late_layers), [`NoiseFilter`](crate::NoiseFilter)
/// and the `EnvFilter` — so noise rules, contributed layers and the
/// `tracing.filter` / `RUST_LOG` directives keep working. It is a plain `fn`
/// (tracing-subscriber cannot box a `FormatEvent`), so it can build any
/// `fmt::layer()…` or a layer of its own:
///
/// ```rust,ignore
/// use r2e_tracing::tracing_subscriber::{fmt, Layer};
/// r2e_tracing::set_console_factory(|_opts| fmt::layer().compact().boxed())?;
/// ```
///
/// The factory owns the console line: it honours (or ignores) the
/// [`TracingOptions`] knobs it receives, and [field enrichers](crate::add_enricher)
/// are **not** applied to the layer it builds.
///
/// Call it **before** the subscriber is installed (in `App::setup`, or before
/// `launch`). A later call returns `Err(AlreadyInstalled)` and stores nothing
/// — also when the subscriber came from elsewhere (then `installed` is
/// `None`). Calling it more than once before the install: the last factory
/// wins.
///
/// The check and the store are atomic with respect to
/// [`try_install`](crate::try_install): `Ok` means the next install uses this
/// factory, never that it was stored after an install already happened. The
/// factory runs with that lock held — it must not call `set_console_factory`
/// itself (deadlock).
pub fn set_console_factory(factory: ConsoleFactory) -> Result<(), AlreadyInstalled> {
    let mut slot = lock_factory();
    if installed().is_some() || tracing::dispatcher::has_been_set() {
        return Err(AlreadyInstalled {
            installed: installed().cloned(),
        });
    }
    *slot = Some(factory);
    Ok(())
}

/// The console layer [`try_install`](crate::try_install) puts in the stack,
/// writing to stdout.
pub(crate) fn console_layer(opts: &TracingOptions) -> BoxedLayer {
    console_layer_with_writer(opts, std::io::stdout)
}

/// The console layer, writing to `make_writer` — `fmt` (JSON under
/// [`LogFormat::Json`], flat when
/// [`JsonOptions::flatten`](crate::JsonOptions::flatten) is set) with the knobs of `opts` and the registered
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
        // Flat JSON writes enricher fields into its own map: no splicing.
        LogFormat::Json if opts.json.flatten => SpanFieldsLayer
            .and_then(layer.event_format(FlatJson::new(opts)))
            .boxed(),
        LogFormat::Json => layer
            .fmt_fields(fmt::format::JsonFields::new())
            .event_format(Enriched::json(format.json()))
            .boxed(),
        LogFormat::Pretty => layer.event_format(Enriched::text(format)).boxed(),
    }
}
