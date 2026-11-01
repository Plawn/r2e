//! A subscriber installed by someone else first wins: capturing is inert and
//! `install` reports an unknown winner.
//!
//! Its **own target**: it sets the process-global subscriber.

use r2e_tracing::{boot_logs_captured, capture_boot_logs, install, installed, TracingOptions};

#[test]
fn a_foreign_subscriber_wins() {
    tracing::subscriber::set_global_default(tracing_subscriber::fmt().finish())
        .expect("first global subscriber");

    let boot = capture_boot_logs();
    assert!(!boot_logs_captured(), "an inert capture captures nothing");
    tracing::info!(target: "foreign_test", "goes to the foreign subscriber");
    drop(boot);

    let lost = install(&TracingOptions::default(), vec![]).expect_err("foreign winner");
    assert!(lost.installed.is_none());
    assert_eq!(installed(), None);
}
