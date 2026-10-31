//! An invalid filter directive does not abort the install: the subscriber is
//! installed with `info` and the error is logged through it.
//!
//! Its **own target**: it installs the process-global subscriber.

use r2e_tracing::{installed, try_install, TracingOptions};

#[test]
fn an_invalid_filter_falls_back_to_info_instead_of_panicking() {
    // SAFETY: single-threaded at this point — the only test of the binary,
    // before anything reads the environment.
    unsafe { std::env::remove_var("RUST_LOG") };

    let opts = TracingOptions::new("info,r2e=loud");
    try_install(&opts).expect("the install still wins");
    // The record keeps what was asked for.
    assert_eq!(installed(), Some(&opts));

    assert!(tracing::enabled!(tracing::Level::INFO));
    assert!(!tracing::enabled!(tracing::Level::DEBUG));
}
