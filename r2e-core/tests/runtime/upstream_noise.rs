//! [`UpstreamNoise`]: the per-layer filter the framework subscriber installs
//! to drop the ERROR events a dependency emits for benign conditions.

use std::sync::{Arc, Mutex};

use r2e_core::UpstreamNoise;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::Registry;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<(Level, String, String)>>>);

struct Message(Option<String>);

impl Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = Some(format!("{value:?}"));
        }
    }
}

impl<S: Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut message = Message(None);
        event.record(&mut message);
        self.0.lock().unwrap().push((
            *event.metadata().level(),
            event.metadata().target().to_string(),
            message.0.unwrap_or_default(),
        ));
    }
}

fn captured(emit: impl FnOnce()) -> Vec<(Level, String, String)> {
    let capture = Capture::default();
    let subscriber = Registry::default().with(capture.clone().with_filter(UpstreamNoise));
    tracing::subscriber::with_default(subscriber, emit);
    capture.0.lock().unwrap().clone()
}

#[test]
fn drops_the_known_rmcp_drain_errors() {
    let events = captured(|| {
        tracing::error!(
            target: "rmcp::service",
            error = "channel closed",
            "failed to send pending response during drain"
        );
        tracing::error!(target: "rmcp::service", error = "channel closed", "fail to response message");
    });
    assert!(events.is_empty(), "suppressed events leaked: {events:?}");
}

#[test]
fn only_the_exact_target_level_and_message_are_dropped() {
    let events = captured(|| {
        // Same message, other target.
        tracing::error!(target: "app::service", "failed to send pending response during drain");
        // Same target, other message.
        tracing::error!(target: "rmcp::service", "response error");
        // Same target and message, lower level: left to the EnvFilter.
        tracing::warn!(target: "rmcp::service", "failed to send pending response during drain");
        // Formatted message that merely contains the suppressed text.
        tracing::error!(target: "rmcp::service", "{} (retrying)", "failed to send pending response during drain");
    });
    let summary: Vec<(Level, &str)> = events
        .iter()
        .map(|(level, target, _)| (*level, target.as_str()))
        .collect();
    assert_eq!(
        summary,
        [
            (Level::ERROR, "app::service"),
            (Level::ERROR, "rmcp::service"),
            (Level::WARN, "rmcp::service"),
            (Level::ERROR, "rmcp::service"),
        ]
    );
}

#[test]
fn suppresses_is_the_public_table() {
    assert!(UpstreamNoise::suppresses(
        "rmcp::service",
        "failed to send pending response during drain"
    ));
    assert!(UpstreamNoise::suppresses(
        "rmcp::service",
        "fail to response message"
    ));
    assert!(!UpstreamNoise::suppresses(
        "rmcp::service",
        "response error"
    ));
    assert!(!UpstreamNoise::suppresses(
        "rmcp",
        "fail to response message"
    ));
}
