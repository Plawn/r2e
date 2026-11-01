//! A hand-written `main` that calls `build_state()` without `launch` (no
//! boot-log capture): what it logged before is dropped, and the install says
//! so in one WARN, next to the INFO summary of the built-in defaults
//! (`source=defaults`).
//!
//! Its **own target**: the global subscriber is a one-shot process global.

use std::sync::{Arc, Mutex};

use r2e_core::{installed, AppBuilder};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

/// One captured event: `"LEVEL target message"`.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<String>>>);

impl Capture {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

impl<S: Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        struct Message(String);
        impl tracing::field::Visit for Message {
            fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
                if f.name() == "message" {
                    self.0 = format!("{v:?}");
                }
            }
        }
        let mut message = Message(String::new());
        event.record(&mut message);
        let meta = event.metadata();
        self.0
            .lock()
            .unwrap()
            .push(format!("{} {} {}", meta.level(), meta.target(), message.0));
    }
}

#[r2e_core::test]
async fn build_state_without_a_capture_warns_that_early_logs_were_dropped() {
    // SAFETY: the only test of the binary, before anything reads the
    // environment. `RUST_LOG` would change the filter in the summary.
    unsafe { std::env::remove_var("RUST_LOG") };

    tracing::info!(target: "no_capture_test", "before build_state");

    let seen = Capture::default();
    let _state = AppBuilder::new()
        .tracing_layer(seen.clone())
        .build_state()
        .await;
    assert!(
        installed().is_some(),
        "build_state installed the subscriber"
    );

    let lines = seen.take();
    assert_eq!(lines.len(), 2, "{lines:?}");
    let summary = &lines[0];
    assert!(
        summary.starts_with(
            "INFO r2e_core::runtime::tracing r2e: tracing installed: format=pretty \
             filter=\"info\" ansi=auto:"
        ),
        "{summary}"
    );
    assert!(
        summary.ends_with(" span_events=close layers=1 source=defaults"),
        "{summary}"
    );
    let warn = &lines[1];
    assert!(
        warn.starts_with("WARN r2e_core::runtime::tracing r2e: no boot-log capture was active"),
        "{warn}"
    );
    assert!(
        warn.contains("r2e::capture_boot_logs()")
            && warn.contains("docs/migration/tracing-single-path.md"),
        "{warn}"
    );

    // A second build keeps the subscriber and says nothing more.
    let _again = AppBuilder::new().build_state().await;
    assert!(seen.take().is_empty());
}
