//! `RUST_LOG=debug`: the boot buffer keeps the DEBUG lines emitted before
//! `install` (config loading, `App::setup`, plugin setup) and the replay
//! prints them; TRACE stays out.
//!
//! Its **own target**: the forwarder is the process-global subscriber and
//! reads `RUST_LOG` once.

#[path = "../support/mod.rs"]
mod support;

use r2e_tracing::{capture_boot_logs, install, SpanEvents, TracingOptions};
use support::Capture;
use tracing::Level;

#[test]
fn rust_log_debug_buffers_and_replays_debug_lines() {
    // SAFETY: single-threaded at this point — the only test of the binary,
    // before anything reads the environment.
    unsafe { std::env::set_var("RUST_LOG", "debug") };
    let _boot = capture_boot_logs();

    assert!(tracing::enabled!(Level::DEBUG));
    tracing::info!(target: "rust_log_test", "info line");
    tracing::debug!(target: "rust_log_test", step = 1, "debug line");
    tracing::trace!(target: "rust_log_test", "trace line");

    // `RUST_LOG` wins over `tracing.filter` at the install too.
    let seen = Capture::default();
    install(
        &TracingOptions::new("info").with_span_events(SpanEvents::None),
        vec![seen.boxed()],
    )
    .expect("the forwarder is ours");

    let summary: Vec<_> = seen
        .take()
        .iter()
        .filter(|e| e.target == "rust_log_test")
        .map(|e| format!("{} {}", e.level, e.message()))
        .collect();
    assert_eq!(summary, ["INFO info line", "DEBUG debug line"]);
}
