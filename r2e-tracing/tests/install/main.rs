//! Installing the process-global subscriber: the first install wins and is
//! recorded, later ones report what won; contributed layers are part of the
//! stack, and a `Suppress` among them gates its siblings.
//!
//! Its **own target**: the global subscriber is a one-shot process global, so
//! who wins the install is a property of the whole binary. One test drives the
//! sequence, in order.

#[path = "../support/mod.rs"]
mod support;

use r2e_tracing::{
    install, installed, AlreadyInstalled, LogFormat, SpanEvents, Suppress, TracingOptions,
};
use support::Capture;
use tracing::Level;

#[test]
fn first_install_wins_and_later_installs_learn_they_lost() {
    assert_eq!(installed(), None, "nothing installed yet");

    let wanted = TracingOptions::new("warn,install_test=debug")
        .with_format(LogFormat::Json)
        .with_span_events(SpanEvents::None)
        // No console: the contributed layers are the whole output.
        .with_console(false);

    // Nobody called `capture_boot_logs`: `install` puts the forwarder in
    // place itself.
    let seen = Capture::default();
    install(
        &wanted,
        vec![
            Box::new(Suppress::error("install_test::noise", "drop me")),
            Box::new(Suppress::new(
                Level::WARN,
                "install_test::noise",
                "drop warn",
            )),
            seen.boxed(),
        ],
    )
    .expect("the first install wins");
    assert_eq!(installed(), Some(&wanted));
    assert!(!installed().unwrap().console, "the console was left out");

    // A contributed layer sees events through the global subscriber, and the
    // `Suppress` layers veto exactly their (level, target, message).
    tracing::error!(target: "install_test::noise", "drop me");
    tracing::error!(target: "install_test::noise", answer = 42, "drop me");
    tracing::error!(target: "install_test::noise", "keep me");
    tracing::warn!(target: "install_test::noise", "drop me");
    tracing::warn!(target: "install_test::noise", "drop warn");
    tracing::error!(target: "install_test::other", "drop me");
    tracing::error!(target: "install_test::noise", "drop me!");
    let messages: Vec<_> = seen
        .take()
        .into_iter()
        .map(|e| format!("{} {} {}", e.level, e.target, e.message()))
        .collect();
    assert_eq!(
        messages,
        [
            "ERROR install_test::noise keep me",
            "WARN install_test::noise drop me",
            "ERROR install_test::other drop me",
            "ERROR install_test::noise drop me!",
        ]
    );

    // The global filter applies to contributed layers.
    tracing::debug!(target: "install_test", "debug is on here");
    tracing::debug!(target: "elsewhere", "but not there");
    let seen_now = seen.take();
    assert_eq!(seen_now.len(), 1, "{seen_now:?}");
    assert_eq!(seen_now[0].message(), "debug is on here");

    // Re-installing the very same knobs is redundant but harmless.
    let lost = install(&wanted, vec![]).expect_err("a subscriber is already installed");
    assert_eq!(lost.installed.as_ref(), Some(&wanted));
    assert!(!lost.changes_output(&wanted));

    // Asking for something else is not harmless.
    let other = wanted.clone().with_format(LogFormat::Pretty);
    let lost = install(&other, vec![]).expect_err("still installed");
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
