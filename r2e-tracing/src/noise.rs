use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{PoisonError, RwLock};

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Metadata, Subscriber};
use tracing_subscriber::layer::{Context, Filter, Layer};

/// One event R2E knows to be benign: an exact `level`, `target` and
/// `message`.
///
/// Registered with [`suppress`] (or [`suppress_error`]) and dropped by
/// [`NoiseFilter`]. `message` is compared with the event's `message` field as
/// `tracing` formats it — exactly, not as a substring.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoiseRule {
    /// The level the event is emitted at; the same message at another level
    /// is left alone.
    pub level: Level,
    /// The event's target (usually the emitting module path).
    pub target: Cow<'static, str>,
    /// The event's formatted `message` field.
    pub message: Cow<'static, str>,
}

/// The process-wide rule table. Append-only.
static RULES: RwLock<Vec<NoiseRule>> = RwLock::new(Vec::new());

/// `true` once a rule exists: an empty table costs one relaxed load per event.
static ANY_RULE: AtomicBool = AtomicBool::new(false);

/// Register a noise rule: from now on [`NoiseFilter`] drops the events it
/// matches.
///
/// Rules are **process-wide** and **append-only** (there is no way to remove
/// one), and the table is empty by default. They exist for upstream bugs R2E
/// knows to be benign — a dependency logging a harmless condition at ERROR —
/// and are registered by the integration that depends on that crate (the
/// `McpServer` plugin registers rmcp's, for instance). Registering the same
/// rule twice is a no-op.
pub fn suppress(rule: NoiseRule) {
    let mut rules = RULES.write().unwrap_or_else(PoisonError::into_inner);
    if !rules.contains(&rule) {
        rules.push(rule);
    }
    ANY_RULE.store(true, Ordering::Release);
}

/// [`suppress`] an ERROR event with this exact `target` and `message`.
pub fn suppress_error(target: impl Into<Cow<'static, str>>, message: impl Into<Cow<'static, str>>) {
    suppress(NoiseRule {
        level: Level::ERROR,
        target: target.into(),
        message: message.into(),
    });
}

/// Whether an event with this `level`, `target` and `message` matches a
/// registered rule.
pub fn is_suppressed(level: &Level, target: &str, message: &str) -> bool {
    if !ANY_RULE.load(Ordering::Relaxed) {
        return false;
    }
    RULES
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .any(|r| r.level == *level && r.target == target && r.message == message)
}

/// Whether some rule watches this `level` + `target` — decided from metadata
/// alone, before paying for the message.
fn watched(level: &Level, target: &str) -> bool {
    if !ANY_RULE.load(Ordering::Relaxed) {
        return false;
    }
    RULES
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .any(|r| r.level == *level && r.target == target)
}

/// Whether `event` matches a registered rule.
fn event_is_noise(event: &Event<'_>) -> bool {
    let meta = event.metadata();
    if !watched(meta.level(), meta.target()) {
        return false;
    }
    // The lock is released while visiting: a field's `Debug` impl may log.
    let mut message = MessageField(None);
    event.record(&mut message);
    message
        .0
        .is_some_and(|m| is_suppressed(meta.level(), meta.target(), &m))
}

/// Drops the events matched by the registered [noise rules](suppress).
///
/// Two ways to use it:
///
/// - as a **global layer** (`impl Layer`): placed above the layers it gates,
///   its `event_enabled` vetoes a matched event for the whole stack. This is
///   how [`try_install`](crate::try_install) uses it — between the console
///   layer + [late layers](crate::late_layers) and the `EnvFilter`, so it
///   gates contributed layers too;
/// - as a **per-layer filter** (`impl Filter`): `layer.with_filter(NoiseFilter)`
///   in a hand-built stack.
///
/// With no rule registered it lets everything through at the cost of one
/// relaxed atomic load per event.
///
/// ```rust,ignore
/// use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
/// tracing_subscriber::registry()
///     .with(tracing_subscriber::fmt::layer())
///     .with(r2e_tracing::NoiseFilter)
///     .with(tracing_subscriber::EnvFilter::from_default_env())
///     .init();
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct NoiseFilter;

impl<S: Subscriber> Layer<S> for NoiseFilter {
    // `register_callsite` keeps the default (`always`): `event_enabled` is
    // consulted on every event whatever the cached interest, so answering
    // `sometimes` would only force a dynamic `enabled` check on every
    // callsite for nothing.

    fn enabled(&self, _meta: &Metadata<'_>, _ctx: Context<'_, S>) -> bool {
        // Metadata alone never decides: the message is a field value.
        true
    }

    fn event_enabled(&self, event: &Event<'_>, _ctx: Context<'_, S>) -> bool {
        !event_is_noise(event)
    }
}

impl<S: Subscriber> Filter<S> for NoiseFilter {
    fn enabled(&self, _meta: &Metadata<'_>, _cx: &Context<'_, S>) -> bool {
        true
    }

    fn event_enabled(&self, event: &Event<'_>, _cx: &Context<'_, S>) -> bool {
        !event_is_noise(event)
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
