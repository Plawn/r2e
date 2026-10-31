//! An invalid filter directive does not abort the install: the subscriber is
//! installed with `info` and the error is logged through it.
//!
//! Its **own target**: it installs the process-global subscriber.

#[path = "../support/mod.rs"]
mod support;

use r2e_tracing::{install, installed, TracingOptions};
use support::Capture;

#[test]
fn an_invalid_filter_falls_back_to_info_instead_of_panicking() {
    // SAFETY: single-threaded at this point — the only test of the binary,
    // before anything reads the environment.
    unsafe { std::env::remove_var("RUST_LOG") };

    let opts = TracingOptions::new("info,r2e=loud");
    let seen = Capture::default();
    install(&opts, vec![seen.boxed()]).expect("the install still wins");
    // The record keeps what was asked for.
    assert_eq!(installed(), Some(&opts));

    assert!(tracing::enabled!(tracing::Level::INFO));
    assert!(!tracing::enabled!(tracing::Level::DEBUG));

    // The parse error went through the installed subscriber.
    let errors: Vec<_> = seen
        .take()
        .into_iter()
        .filter(|e| e.level == tracing::Level::ERROR)
        .collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].message().contains("invalid tracing filter"),
        "{errors:?}"
    );
}
