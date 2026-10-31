use tracing::field::{Field, Visit};
use tracing::{Event, Level, Metadata, Subscriber};
use tracing_subscriber::layer::{Context, Filter};

/// Per-layer filter that drops the ERROR events a dependency emits for
/// conditions R2E knows to be benign.
///
/// `EnvFilter` directives cannot express "everything from this target except
/// *this* message", and the message is only known at event time, so the
/// framework subscriber installs this filter on its `fmt` layer next to the
/// `EnvFilter`. Each suppressed `(target, message)` pair is a known upstream
/// bug; the entry is removed once the upstream release that fixes it becomes
/// R2E's minimum. Only ERROR-level events are matched: the same message at
/// WARN or below is left to the `EnvFilter`.
///
/// Suppressed today:
///
/// - `rmcp::service` — `failed to send pending response during drain` and
///   `fail to response message` (rmcp ≤ 3.8). In stateless streamable HTTP
///   every request runs its own service over a oneshot channel; when the
///   client disconnects first (a `subscriptions/listen` SSE stream the client
///   closes after its idle period, a cancelled tool call) the service is
///   cancelled, drains, and logs the closed channel at ERROR. Nothing leaks
///   and nothing is lost: the response has no reader. Tracked as #1185.
///
/// Available to applications installing their own subscriber:
///
/// ```rust,ignore
/// use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, Layer};
/// tracing_subscriber::registry()
///     .with(tracing_subscriber::fmt::layer().with_filter(r2e_tracing::UpstreamNoise))
///     .with(tracing_subscriber::EnvFilter::from_default_env())
///     .init();
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct UpstreamNoise;

/// `(target, message)` of the ERROR events [`UpstreamNoise`] drops.
const SUPPRESSED: &[(&str, &str)] = &[
    (
        "rmcp::service",
        "failed to send pending response during drain",
    ),
    ("rmcp::service", "fail to response message"),
];

impl UpstreamNoise {
    /// Whether an ERROR event with this `target` and `message` is suppressed.
    pub fn suppresses(target: &str, message: &str) -> bool {
        SUPPRESSED
            .iter()
            .any(|(t, m)| *t == target && *m == message)
    }

    fn target_watched(target: &str) -> bool {
        SUPPRESSED.iter().any(|(t, _)| *t == target)
    }
}

impl<S: Subscriber> Filter<S> for UpstreamNoise {
    fn enabled(&self, _meta: &Metadata<'_>, _cx: &Context<'_, S>) -> bool {
        // Metadata alone never decides: the message is a field value.
        true
    }

    fn event_enabled(&self, event: &Event<'_>, _cx: &Context<'_, S>) -> bool {
        let meta = event.metadata();
        if *meta.level() != Level::ERROR || !Self::target_watched(meta.target()) {
            return true;
        }
        let mut message = MessageField(None);
        event.record(&mut message);
        !message
            .0
            .is_some_and(|m| Self::suppresses(meta.target(), &m))
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
