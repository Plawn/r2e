//! The boot buffer is bounded: past 1024 events the rest is counted and
//! reported by one WARN after the replay.
//!
//! Its **own target**: the forwarder is the process-global subscriber.

#[path = "../support/mod.rs"]
mod support;

use r2e_tracing::{capture_boot_logs, install, SpanEvents, TracingOptions};
use support::Capture;
use tracing::Level;

#[test]
fn the_buffer_keeps_1024_events_and_reports_the_rest() {
    let _boot = capture_boot_logs();
    for i in 0..1100 {
        tracing::info!(target: "bound_test", i, "boot line");
    }

    let seen = Capture::default();
    install(
        &TracingOptions::new("info").with_span_events(SpanEvents::None),
        vec![seen.boxed()],
    )
    .expect("the forwarder is ours");

    let all = seen.take();
    let replayed: Vec<_> = all.iter().filter(|e| e.target == "bound_test").collect();
    assert_eq!(replayed.len(), 1024);
    assert_eq!(replayed[0].field("i"), Some("0"));
    assert_eq!(replayed[1023].field("i"), Some("1023"));

    let warns: Vec<_> = all.iter().filter(|e| e.level == Level::WARN).collect();
    assert_eq!(warns.len(), 1, "{warns:?}");
    assert_eq!(warns[0].field("dropped"), Some("76"));
    assert!(warns[0].message().contains("dropped"), "{warns:?}");
    // After the replay.
    assert_eq!(all.last().unwrap().level, Level::WARN);
}
