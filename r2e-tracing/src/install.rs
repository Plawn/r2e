use std::sync::OnceLock;

use tracing_subscriber::layer::{Layer, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter, Registry};

use crate::noise::UpstreamNoise;
use crate::options::{LogFormat, TracingOptions};

/// The options the process-global subscriber was actually installed with,
/// when R2E installed it.
///
/// A `tracing` subscriber is a one-shot process global: the first install
/// wins and every later one is a no-op. Recording what won lets a losing
/// caller tell the two cases apart — re-installing the *same* knobs (an
/// entry point and a plugin both reading the same `tracing:` section, or
/// `#[r2e::test]` installing once per test) is redundant but harmless,
/// whereas losing with *different* knobs means the output the caller asked
/// for is silently not the output the process produces.
static INSTALLED: OnceLock<TracingOptions> = OnceLock::new();

/// The options the process-global subscriber was installed with, when
/// [`try_install`] installed it; `None` when nothing is installed yet or the
/// subscriber came from elsewhere (the application, a test harness, another
/// library).
pub fn installed() -> Option<&'static TracingOptions> {
    INSTALLED.get()
}

/// An install that lost the race to a subscriber installed earlier.
///
/// `installed` carries the winning options when R2E installed it, and is
/// `None` when the subscriber came from elsewhere.
#[derive(Debug, Clone)]
pub struct AlreadyInstalled {
    /// The options that actually won, when they are known.
    pub installed: Option<TracingOptions>,
}

impl AlreadyInstalled {
    /// Whether the winning subscriber differs from what `wanted` asked for —
    /// i.e. whether losing the race actually changed the output.
    ///
    /// An unknown winner counts as different: nothing says it honours the
    /// requested format or filter.
    pub fn changes_output(&self, wanted: &TracingOptions) -> bool {
        self.installed.as_ref() != Some(wanted)
    }
}

impl std::fmt::Display for AlreadyInstalled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.installed {
            Some(o) => write!(
                f,
                "a global tracing subscriber is already installed (format={:?}, filter={:?})",
                o.effective_format(),
                o.filter
            ),
            None => write!(f, "a global tracing subscriber is already installed"),
        }
    }
}

impl std::error::Error for AlreadyInstalled {}

/// Install the process-global `tracing` subscriber built from `opts`.
///
/// The stack is `Registry` + a `fmt` layer (JSON when
/// [`LogFormat::Json`]) carrying the per-layer [`UpstreamNoise`] filter, with
/// an `EnvFilter` as the outer, global layer. `RUST_LOG` takes priority over
/// `opts.filter`.
///
/// `Err(AlreadyInstalled)` means a subscriber was already in place and these
/// options had no effect at all. On success the options are recorded and
/// [`installed`] returns them.
pub fn try_install(opts: &TracingOptions) -> Result<(), AlreadyInstalled> {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| opts.filter.parse().unwrap());

    let span_events = opts.effective_span_events();
    let target = opts.target.unwrap_or(true);
    let thread_ids = opts.thread_ids.unwrap_or(false);
    let thread_names = opts.thread_names.unwrap_or(false);
    let file = opts.file.unwrap_or(false);
    let line_number = opts.line_number.unwrap_or(false);
    let level = opts.level.unwrap_or(true);
    let ansi = opts.ansi.unwrap_or(true);

    // Same stack `tracing_subscriber::fmt().with_env_filter(..).try_init()`
    // builds — `EnvFilter` as the outer, global layer — plus the framework's
    // per-layer [`UpstreamNoise`] filter on the `fmt` layer.
    let fmt_layer = fmt::layer()
        .with_target(target)
        .with_thread_ids(thread_ids)
        .with_thread_names(thread_names)
        .with_file(file)
        .with_line_number(line_number)
        .with_level(level)
        .with_ansi(ansi)
        .with_span_events(span_events);
    let outcome = match opts.effective_format() {
        LogFormat::Json => Registry::default()
            .with(fmt_layer.json().with_filter(UpstreamNoise))
            .with(env_filter)
            .try_init(),
        LogFormat::Pretty => Registry::default()
            .with(fmt_layer.with_filter(UpstreamNoise))
            .with(env_filter)
            .try_init(),
    };

    match outcome {
        Ok(()) => {
            // Best-effort: the race this loses is another thread installing
            // at the same instant, and then it is that one's options that are
            // recorded — which is exactly what we want to remember.
            let _ = INSTALLED.set(opts.clone());
            Ok(())
        }
        Err(_) => Err(AlreadyInstalled {
            installed: INSTALLED.get().cloned(),
        }),
    }
}
