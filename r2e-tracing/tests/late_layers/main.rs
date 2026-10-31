//! The late-layer slot: layers contributed after the subscriber is installed.
//!
//! Its **own target**: the slot is wired by the process-global install, so the
//! whole binary is one ordered sequence — `NotWired` before the install, then
//! contributions into the installed subscriber.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use r2e_tracing::{contribute_layer, late_layers, try_install, NotWired, TracingOptions};
use tracing::subscriber::Interest;
use tracing::{span, Event, Metadata};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::Registry;

const TARGET: &str = "late_layers_test";

/// Counts what reaches it, for `TARGET` only (other events — the install's
/// own — are ignored).
#[derive(Clone, Default)]
struct Capture {
    events: Arc<AtomicUsize>,
    new_spans: Arc<AtomicUsize>,
    enters: Arc<AtomicUsize>,
    closes: Arc<AtomicUsize>,
}

impl Layer<Registry> for Capture {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, Registry>) {
        if event.metadata().target() == TARGET {
            self.events.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn on_new_span(
        &self,
        attrs: &span::Attributes<'_>,
        _id: &span::Id,
        _ctx: Context<'_, Registry>,
    ) {
        if attrs.metadata().target() == TARGET {
            self.new_spans.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn on_enter(&self, id: &span::Id, ctx: Context<'_, Registry>) {
        if ctx.metadata(id).is_some_and(|m| m.target() == TARGET) {
            self.enters.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn on_close(&self, id: span::Id, ctx: Context<'_, Registry>) {
        if ctx.metadata(&id).is_some_and(|m| m.target() == TARGET) {
            self.closes.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// Not interested in anything: must neither see events nor veto the slot.
#[derive(Clone, Default)]
struct Never {
    events: Arc<AtomicUsize>,
}

impl Layer<Registry> for Never {
    fn register_callsite(&self, _metadata: &'static Metadata<'static>) -> Interest {
        Interest::never()
    }

    fn enabled(&self, _metadata: &Metadata<'_>, _ctx: Context<'_, Registry>) -> bool {
        false
    }

    fn on_event(&self, _event: &Event<'_>, _ctx: Context<'_, Registry>) {
        self.events.fetch_add(1, Ordering::SeqCst);
    }
}

/// Found through the dispatch by `downcast_ref`.
struct Marker;

impl Layer<Registry> for Marker {}

#[test]
fn contributed_layers_join_the_installed_subscriber() {
    // Before any install, nothing hosts the slot.
    assert_eq!(
        contribute_layer(Box::new(Capture::default())),
        Err(NotWired)
    );
    assert!(NotWired.to_string().contains("late_layers"));

    try_install(&TracingOptions::new("trace")).expect("first install");

    // A layer that wants nothing goes in first: it must not make the slot
    // `never` for callsites a later layer wants.
    let never = Never::default();
    contribute_layer(Box::new(never.clone())).expect("wired after install");

    let capture = Capture::default();
    contribute_layer(Box::new(capture.clone())).expect("wired after install");
    contribute_layer(Box::new(Marker)).expect("wired after install");

    // Events and the whole span lifecycle reach the contributed layer.
    {
        let span = tracing::info_span!(target: "late_layers_test", "work");
        let _entered = span.enter();
        tracing::info!(target: "late_layers_test", "inside");
    }
    tracing::warn!(target: "late_layers_test", "outside");

    assert_eq!(capture.events.load(Ordering::SeqCst), 2);
    assert_eq!(capture.new_spans.load(Ordering::SeqCst), 1);
    assert_eq!(capture.enters.load(Ordering::SeqCst), 1);
    assert_eq!(capture.closes.load(Ordering::SeqCst), 1);

    // A child's own `enabled` is honoured for that child only.
    assert_eq!(never.events.load(Ordering::SeqCst), 0);

    // The slot's interest is the max over its children: `always` wins over
    // `never`.
    let span = tracing::info_span!(target: "late_layers_test", "probe");
    let meta = span.metadata().expect("an enabled span has metadata");
    assert!(late_layers().register_callsite(meta).is_always());

    // `downcast_ref` reaches a contributed layer through the dispatch.
    tracing::dispatcher::get_default(|dispatch| {
        assert!(dispatch.downcast_ref::<Marker>().is_some());
        assert!(dispatch.downcast_ref::<r2e_tracing::LateLayers>().is_some());
    });
}
