//! Installing the process-global subscriber: the first install wins and is
//! recorded, later ones report what won.
//!
//! Its **own target**: the global subscriber is a one-shot process global, so
//! who wins the install is a property of the whole binary. One test drives the
//! sequence, in order.

use r2e_tracing::{
    installed, try_install, AlreadyInstalled, LogFormat, SpanEvents, TracingOptions,
};

#[test]
fn first_install_wins_and_later_installs_learn_they_lost() {
    assert_eq!(installed(), None, "nothing installed yet");

    let wanted = TracingOptions::new("warn,r2e=debug")
        .with_format(LogFormat::Json)
        .with_span_events(SpanEvents::Full);

    try_install(&wanted).expect("the first install wins");
    assert_eq!(installed(), Some(&wanted));

    // Re-installing the very same knobs is redundant but harmless.
    let lost = try_install(&wanted).expect_err("a subscriber is already installed");
    assert_eq!(lost.installed.as_ref(), Some(&wanted));
    assert!(!lost.changes_output(&wanted));

    // Asking for something else is not harmless.
    let other = wanted.clone().with_format(LogFormat::Pretty);
    let lost = try_install(&other).expect_err("still installed");
    assert!(lost.changes_output(&other));
    assert!(lost.to_string().contains("Json"), "{lost}");

    // The record never moves.
    assert_eq!(installed(), Some(&wanted));
}

/// An unknown winner (a subscriber installed by the app or another library)
/// counts as a difference: nothing says it honours what was requested.
#[test]
fn an_unknown_winner_always_counts_as_a_difference() {
    let lost = AlreadyInstalled { installed: None };
    assert!(lost.changes_output(&TracingOptions::default()));
    assert!(lost.to_string().contains("already"));
}
