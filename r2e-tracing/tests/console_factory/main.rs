//! A custom console factory replaces the console layer of the installed
//! subscriber, and nothing else.
//!
//! Its **own target**: the factory is read by the process-global install, so
//! the whole binary is one ordered sequence — register the factory, install,
//! then observe.

use std::fmt;
use std::io::{self, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use r2e_tracing::{
    contribute_layer, installed, set_console_factory, try_install, BoxedLayer, TracingOptions,
};
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields, MakeWriter};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Registry;

const TARGET: &str = "console_factory_test";
const MARKER: &str = "CUSTOM-CONSOLE|";

/// Everything the custom console layer writes. A static: the factory is a
/// plain `fn` and cannot capture.
static OUTPUT: Mutex<Vec<u8>> = Mutex::new(Vec::new());

fn output() -> String {
    String::from_utf8(OUTPUT.lock().unwrap().clone()).unwrap()
}

struct Capture;

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        OUTPUT.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Capture;

    fn make_writer(&'a self) -> Capture {
        Capture
    }
}

/// `MARKER` then the event's fields — nothing of the default format (no
/// timestamp, no level, no target).
struct Marked;

impl<S, N> FormatEvent<S, N> for Marked
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        write!(writer, "{MARKER}")?;
        ctx.field_format().format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}

fn custom_console(_opts: &TracingOptions) -> BoxedLayer {
    tracing_subscriber::fmt::layer()
        .with_writer(Capture)
        .event_format(Marked)
        .boxed()
}

/// Replaced by `custom_console` before the install: last one wins.
fn never_used(_opts: &TracingOptions) -> BoxedLayer {
    panic!("a factory replaced before the install must not be called")
}

/// Counts the `TARGET` events that reach it.
#[derive(Clone, Default)]
struct Counter(Arc<AtomicUsize>);

impl Layer<Registry> for Counter {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, Registry>) {
        if event.metadata().target() == TARGET {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[test]
fn the_factory_replaces_the_console_layer_and_only_it() {
    // (a) Registered before the install, the factory builds the console.
    set_console_factory(never_used).expect("nothing installed yet");
    set_console_factory(custom_console).expect("nothing installed yet");

    let wanted = TracingOptions::new("info");
    try_install(&wanted).expect("the first install wins");

    tracing::info!(target: TARGET, "hello from the custom console");
    let out = output();
    assert!(
        out.contains(&format!("{MARKER}hello from the custom console")),
        "{out:?}"
    );
    assert!(!out.contains(TARGET), "the default format leaked: {out:?}");
    assert!(!out.contains("INFO"), "the default format leaked: {out:?}");

    // (b) After the install, a factory is refused and names the winner.
    let lost = set_console_factory(never_used).expect_err("already installed");
    assert_eq!(lost.installed.as_ref(), Some(&wanted));
    assert_eq!(installed(), Some(&wanted));

    // (c) The late-layer slot is still in the stack.
    let counter = Counter::default();
    contribute_layer(Box::new(counter.clone())).expect("the slot is wired");
    tracing::info!(target: TARGET, "seen by the contributed layer");
    assert_eq!(counter.0.load(Ordering::SeqCst), 1);
    assert!(output().contains(&format!("{MARKER}seen by the contributed layer")));
}
