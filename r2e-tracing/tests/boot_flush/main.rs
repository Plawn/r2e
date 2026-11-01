//! A boot that fails before `install`: the buffered lines are written raw
//! (to stderr when the `BootLogs` handle drops), and a later install still
//! works.
//!
//! Its **own target**: the forwarder is the process-global subscriber.

#[path = "../support/mod.rs"]
mod support;

use r2e_tracing::{capture_boot_logs, flush_boot_logs_to, install, SpanEvents, TracingOptions};
use support::Capture;

#[test]
fn buffered_lines_are_flushed_raw_and_install_still_works() {
    // SAFETY: single-threaded at this point — the only test of the binary,
    // before anything reads the environment. The boot filter follows
    // `RUST_LOG`; this scenario is about the default (INFO).
    unsafe { std::env::remove_var("RUST_LOG") };
    let boot = capture_boot_logs();
    tracing::info!(target: "flush_test", port = 3000, "starting");
    tracing::error!(target: "flush_test::config", key = "db.url", "missing key");
    tracing::debug!(target: "flush_test", "not buffered");

    let mut out = Vec::new();
    flush_boot_logs_to(&mut out);
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "INFO flush_test: starting port=3000\n\
         ERROR flush_test::config: missing key key=db.url\n"
    );

    // The buffer is empty afterwards.
    let mut again = Vec::new();
    flush_boot_logs_to(&mut again);
    assert!(again.is_empty());

    // Still buffering: a dropped handle flushes (nothing left here) and a
    // later capture + install proceed normally.
    drop(boot);
    let _boot = capture_boot_logs();
    tracing::warn!(target: "flush_test", "second boot");

    let seen = Capture::default();
    install(
        &TracingOptions::new("info").with_span_events(SpanEvents::None),
        vec![seen.boxed()],
    )
    .expect("the forwarder is ours");
    let replayed: Vec<_> = seen
        .take()
        .into_iter()
        .map(|e| e.message().to_owned())
        .collect();
    assert_eq!(replayed, ["second boot"]);
}
