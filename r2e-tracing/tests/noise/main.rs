//! Noise rules: the process-wide table and [`NoiseFilter`], as a global layer
//! and as a per-layer filter.
//!
//! Rules are process-wide and append-only, so every test uses its own target:
//! a rule registered by one test never matches another's events. Subscribers
//! are scoped (`with_default`), never global.

use std::sync::{Arc, Mutex};

use r2e_tracing::{is_suppressed, suppress, suppress_error, NoiseFilter, NoiseRule};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::Registry;

/// Records the `message` of every event it sees.
#[derive(Clone, Default)]
struct Messages(Arc<Mutex<Vec<String>>>);

impl Messages {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut self.0.lock().unwrap())
    }
}

impl<S: Subscriber> Layer<S> for Messages {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        struct V(String);
        impl tracing::field::Visit for V {
            fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
                if f.name() == "message" {
                    self.0 = format!("{v:?}");
                }
            }
        }
        let mut v = V(String::new());
        event.record(&mut v);
        self.0.lock().unwrap().push(v.0);
    }
}

#[test]
fn rules_match_level_target_and_message_exactly() {
    let target = "noise_test::exact";
    assert!(!is_suppressed(&Level::ERROR, target, "boom"));

    suppress_error(target, "boom");
    assert!(is_suppressed(&Level::ERROR, target, "boom"));
    // Exact: another level, another target, a longer message are left alone.
    assert!(!is_suppressed(&Level::WARN, target, "boom"));
    assert!(!is_suppressed(&Level::ERROR, "noise_test::other", "boom"));
    assert!(!is_suppressed(&Level::ERROR, target, "boom!"));
    assert!(!is_suppressed(&Level::ERROR, target, "boo"));
}

#[test]
fn suppress_is_idempotent_and_takes_any_level() {
    let target = "noise_test::idempotent";
    let rule = NoiseRule {
        level: Level::WARN,
        target: target.into(),
        message: "flaky upstream".into(),
    };
    suppress(rule.clone());
    suppress(rule);
    assert!(is_suppressed(&Level::WARN, target, "flaky upstream"));
    assert!(!is_suppressed(&Level::ERROR, target, "flaky upstream"));
}

#[test]
fn as_a_global_layer_it_drops_matched_events_for_the_stack() {
    let target = "noise_test::layer";
    suppress_error(target, "drop me");
    let seen = Messages::default();
    let subscriber = Registry::default().with(seen.clone()).with(NoiseFilter);

    tracing::subscriber::with_default(subscriber, || {
        tracing::error!(target: "noise_test::layer", "drop me");
        tracing::error!(target: "noise_test::layer", "keep me");
        tracing::warn!(target: "noise_test::layer", "drop me");
        tracing::error!(target: "noise_test::layer", answer = 42, "drop me");
    });

    // Fields besides the message do not defeat the match.
    assert_eq!(seen.take(), ["keep me", "drop me"]);
}

#[test]
fn as_a_per_layer_filter_it_gates_only_its_layer() {
    let target = "noise_test::filter";
    suppress_error(target, "drop me");
    let filtered = Messages::default();
    let unfiltered = Messages::default();
    let subscriber = Registry::default()
        .with(filtered.clone().with_filter(NoiseFilter))
        .with(unfiltered.clone());

    tracing::subscriber::with_default(subscriber, || {
        tracing::error!(target: "noise_test::filter", "drop me");
        tracing::error!(target: "noise_test::filter", "keep me");
    });

    assert_eq!(filtered.take(), ["keep me"]);
    assert_eq!(unfiltered.take(), ["drop me", "keep me"]);
}

#[test]
fn events_outside_watched_targets_pass_through() {
    suppress_error("noise_test::watched", "drop me");
    let seen = Messages::default();
    let subscriber = Registry::default().with(seen.clone()).with(NoiseFilter);

    tracing::subscriber::with_default(subscriber, || {
        tracing::error!(target: "noise_test::unwatched", "drop me");
    });

    assert_eq!(seen.take(), ["drop me"]);
}
