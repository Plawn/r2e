use std::borrow::Cow;

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

/// A layer that drops one event R2E knows to be benign: an exact `level`,
/// `target` and `message`.
///
/// `message` is compared with the event's `message` field as `tracing`
/// formats it — exactly, not as a substring; other fields do not defeat the
/// match. It exists for upstream bugs an integration knows to be harmless (a
/// dependency logging a benign condition at ERROR), and is contributed by that
/// integration through `tracing_layer` like any other layer.
///
/// It vetoes in `event_enabled`, which is **conjunctive** across the stack
/// [`install`](crate::install) builds: a matched event reaches no layer at all
/// — not the console, not a sibling contributed layer. It never filters at
/// callsite level (the message is a field value, so every event of the
/// callsite has to be inspected).
///
/// ```rust,ignore
/// ctx.tracing_layer(r2e_tracing::Suppress::error("rmcp::transport", "worker quit"));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suppress {
    level: Level,
    target: Cow<'static, str>,
    message: Cow<'static, str>,
}

impl Suppress {
    /// Drop events with this exact `level`, `target` and `message`.
    pub fn new(
        level: Level,
        target: impl Into<Cow<'static, str>>,
        message: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            level,
            target: target.into(),
            message: message.into(),
        }
    }

    /// Drop ERROR events with this exact `target` and `message`.
    pub fn error(
        target: impl Into<Cow<'static, str>>,
        message: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self::new(Level::ERROR, target, message)
    }

    fn matches(&self, event: &Event<'_>) -> bool {
        let meta = event.metadata();
        // Metadata first: the message is only rendered for a watched callsite.
        if *meta.level() != self.level || meta.target() != self.target {
            return false;
        }
        let mut message = MessageField(None);
        event.record(&mut message);
        message.0.is_some_and(|m| m == self.message)
    }
}

impl<S: Subscriber> Layer<S> for Suppress {
    // `register_callsite` / `enabled` keep their defaults (`always` / `true`):
    // metadata alone never decides, and `event_enabled` is consulted on every
    // event whatever the cached interest.

    fn event_enabled(&self, event: &Event<'_>, _ctx: Context<'_, S>) -> bool {
        !self.matches(event)
    }
}

/// Collects the `message` field of an event, as `tracing` formats it.
struct MessageField(Option<String>);

impl Visit for MessageField {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = Some(format!("{value:?}"));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0 = Some(value.to_string());
        }
    }
}
