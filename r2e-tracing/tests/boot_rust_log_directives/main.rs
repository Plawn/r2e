//! A directive-scoped `RUST_LOG` decides what the boot buffer keeps per
//! target, and the replay still goes through the *installed* filter.
//!
//! Its **own target**: the forwarder is the process-global subscriber and
//! reads `RUST_LOG` once.

#[path = "../support/mod.rs"]
mod support;

use r2e_tracing::{capture_boot_logs, install, SpanEvents, TracingOptions};
use support::Capture;

#[test]
fn directives_scope_the_buffer_and_the_installed_filter_scopes_the_replay() {
    // SAFETY: single-threaded at this point — the only test of the binary,
    // before anything reads the environment.
    unsafe { std::env::set_var("RUST_LOG", "info,boot_scoped=debug") };
    let _boot = capture_boot_logs();

    tracing::debug!(target: "boot_scoped", "kept");
    tracing::trace!(target: "boot_scoped", "trace not buffered");
    tracing::debug!(target: "boot_scoped::deny", "denied at replay");
    tracing::debug!(target: "boot_other", "other debug not buffered");
    tracing::info!(target: "boot_other", "other info");

    // Install under a different filter than the one the buffer used: more
    // permissive for `boot_scoped` and `boot_other` (so whatever is missing
    // from the replay was never buffered), stricter for `boot_scoped::deny`
    // (so a buffered line the installed filter rejects must not be printed).
    // SAFETY: still the only thread of this test touching the environment.
    unsafe { std::env::remove_var("RUST_LOG") };
    let seen = Capture::default();
    install(
        &TracingOptions::new("info,boot_scoped=trace,boot_scoped::deny=info,boot_other=trace")
            .with_span_events(SpanEvents::None),
        vec![seen.boxed()],
    )
    .expect("the forwarder is ours");

    let ours = |e: &support::Seen| e.target.starts_with("boot_");
    let summary: Vec<_> = seen
        .take()
        .iter()
        .filter(|e| ours(e))
        .map(|e| format!("{} {} {}", e.level, e.target, e.message()))
        .collect();
    assert_eq!(
        summary,
        ["DEBUG boot_scoped kept", "INFO boot_other other info"],
        "boot_scoped DEBUG only; boot_scoped::deny filtered at replay"
    );

    // The same callsites live: the installed filter lets TRACE for
    // `boot_scoped` and DEBUG for `boot_other` through — their absence above
    // came from the buffer — and still rejects `boot_scoped::deny` DEBUG.
    tracing::trace!(target: "boot_scoped", "live trace");
    tracing::debug!(target: "boot_scoped::deny", "live denied");
    tracing::debug!(target: "boot_other", "live other debug");
    let live: Vec<_> = seen
        .take()
        .iter()
        .filter(|e| ours(e))
        .map(|e| e.message().to_owned())
        .collect();
    assert_eq!(live, ["live trace", "live other debug"]);
}
