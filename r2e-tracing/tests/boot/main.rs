//! The boot forwarder: events emitted before `install` are buffered and
//! replayed through the real stack, spans straddling the install are safe,
//! and the interest cache is rebuilt on the switch.
//!
//! Its **own target**: the forwarder is the process-global subscriber. One
//! test drives the sequence, in order.

#[path = "../support/mod.rs"]
mod support;

use r2e_tracing::{capture_boot_logs, install, installed, SpanEvents, TracingOptions};
use support::Capture;
use tracing::Level;

fn debug_hit(n: u32) {
    tracing::debug!(target: "boot_test", n, "debug callsite");
}

#[test]
fn boot_logs_are_replayed_then_forwarded() {
    // SAFETY: single-threaded at this point — the only test of the binary,
    // before anything reads the environment. The boot filter follows
    // `RUST_LOG`; this scenario is about the default (INFO).
    unsafe { std::env::remove_var("RUST_LOG") };
    let boot = capture_boot_logs();

    tracing::info!(target: "boot_test", count = 3, who = "me", "first");
    tracing::debug!(target: "boot_test", "never buffered");
    tracing::warn!(target: "boot_test::inner", ok = true, "second");
    debug_hit(0);

    // A span entered before the install and left after it — what an
    // `#[instrument]`ed `App::build` does.
    let straddling = tracing::info_span!("straddling", phase = "build");
    let entered = straddling.clone().entered();
    tracing::error!(target: "boot_test", "inside the boot span");

    let seen = Capture::default();
    install(
        &TracingOptions::new("debug").with_span_events(SpanEvents::None),
        vec![seen.boxed()],
    )
    .expect("the forwarder is ours");
    assert!(installed().is_some());

    let replayed = seen.take();
    let summary: Vec<_> = replayed
        .iter()
        .map(|e| format!("{} {} {}", e.level, e.target, e.message()))
        .collect();
    assert_eq!(
        summary,
        [
            "INFO boot_test first",
            "WARN boot_test::inner second",
            "ERROR boot_test inside the boot span",
        ],
        "INFO and above, in order; DEBUG was never buffered"
    );
    assert_eq!(replayed[0].field("count"), Some("3"));
    assert_eq!(replayed[0].field("who"), Some("me"));
    assert_eq!(replayed[1].field("ok"), Some("true"));
    assert_eq!(replayed[1].level, Level::WARN);

    // Live from now on.
    tracing::info!(target: "boot_test", "live");
    let live = seen.take();
    assert_eq!(live.len(), 1, "{live:?}");
    assert_eq!(live[0].message(), "live");

    // The interest cache was rebuilt: `LevelFilter::current()` was INFO while
    // buffering, so a DEBUG callsite would stay off without the rebuild.
    assert!(tracing::enabled!(Level::DEBUG));
    debug_hit(1);
    let debug = seen.take();
    assert_eq!(debug.len(), 1, "{debug:?}");
    assert_eq!(debug[0].field("n"), Some("1"));

    // The pre-install span: a child with it as explicit parent, a record, a
    // clone, exiting and closing it — none reaches the real registry, none
    // panics.
    let child = tracing::info_span!(parent: &straddling, "child");
    child.in_scope(|| tracing::info!(target: "boot_test", "in child"));
    assert_eq!(seen.take().len(), 1);
    straddling.record("phase", "served");
    let again = straddling.clone();
    drop(entered);
    drop(again);
    drop(straddling);
    drop(child);

    // Spans opened after the install work as usual.
    tracing::info_span!("after").in_scope(|| tracing::info!(target: "boot_test", "in after"));
    assert_eq!(seen.take().len(), 1);

    // Dropping the handle after the install writes nothing and changes
    // nothing.
    drop(boot);
    tracing::info!(target: "boot_test", "still live");
    assert_eq!(seen.take().len(), 1);
}
