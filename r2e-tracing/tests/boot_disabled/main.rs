//! `discard_boot_logs` (`tracing.enabled: false`): the buffer is dropped and
//! every callsite is off; a later install still turns logging on, without
//! replaying anything.
//!
//! Its **own target**: the forwarder is the process-global subscriber.

#[path = "../support/mod.rs"]
mod support;

use r2e_tracing::{
    capture_boot_logs, discard_boot_logs, flush_boot_logs_to, install, installed, SpanEvents,
    TracingOptions,
};
use support::Capture;
use tracing::Level;

#[test]
fn discarded_boot_logs_are_gone_and_logging_is_off() {
    let boot = capture_boot_logs();
    tracing::info!(target: "disabled_test", "buffered then discarded");

    discard_boot_logs();
    assert_eq!(installed(), None);
    assert!(!tracing::enabled!(Level::ERROR));
    tracing::error!(target: "disabled_test", "dropped while disabled");

    let mut out = Vec::new();
    flush_boot_logs_to(&mut out);
    assert!(out.is_empty(), "{}", String::from_utf8_lossy(&out));
    drop(boot);

    let seen = Capture::default();
    install(
        &TracingOptions::new("info").with_span_events(SpanEvents::None),
        vec![seen.boxed()],
    )
    .expect("install after discard is allowed");
    assert!(seen.take().is_empty(), "nothing replayed");

    tracing::info!(target: "disabled_test", "live");
    let live = seen.take();
    assert_eq!(live.len(), 1, "{live:?}");
}
