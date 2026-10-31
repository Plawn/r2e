use std::sync::OnceLock;

use tracing::Dispatch;
use tracing_subscriber::layer::{Layer, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry};

use crate::console::{console_layer, lock_factory};
use crate::late::{self, late_layers};
use crate::noise::NoiseFilter;
use crate::options::TracingOptions;

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
/// The stack is
///
/// ```text
/// Registry
///   + [console layer, late-layer slot]   // fmt (JSON under LogFormat::Json), or the
///                                        // set_console_factory layer + contributed layers
///   + NoiseFilter                        // registered noise rules, for every inner layer
///   + EnvFilter                          // outer, global
/// ```
///
/// `RUST_LOG`, when set and valid, takes priority over `opts.filter`. An
/// invalid `opts.filter` does not fail the install: the subscriber falls back
/// to `"info"` and logs an ERROR naming the directive. With `opts.ansi` unset,
/// colours are on only when stdout is a terminal.
///
/// `Err(AlreadyInstalled)` means a subscriber was already in place and these
/// options had no effect at all. On success the options are recorded and
/// [`installed`] returns them, and the [late-layer slot](crate::late_layers)
/// accepts [contributions](crate::contribute_layer).
pub fn try_install(opts: &TracingOptions) -> Result<(), AlreadyInstalled> {
    // Held for the whole install: a concurrent `set_console_factory` either
    // lands before (and is used here) or after (and sees the install).
    let factory = lock_factory();
    let mut invalid_filter = None;
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::try_new(&opts.filter).unwrap_or_else(|e| {
            invalid_filter = Some(e);
            EnvFilter::new("info")
        })
    });

    let stack = Registry::default()
        .with(vec![
            factory.map_or_else(|| console_layer(opts), |make| make(opts)),
            late_layers().boxed(),
        ])
        .with(NoiseFilter)
        .with(env_filter);

    // Registered by hand rather than through `try_init` on the stack: the
    // `Vec` does not forward `on_register_dispatch`, and the late-layer slot
    // must be wired to *this* dispatch only once it has won the install.
    let dispatch = Dispatch::new(stack);
    match dispatch.clone().try_init() {
        Ok(()) => {
            late::wire(&dispatch);
            // Best-effort: the race this loses is another thread installing
            // at the same instant, and then it is that one's options that are
            // recorded — which is exactly what we want to remember.
            let _ = INSTALLED.set(opts.clone());
            if let Some(e) = invalid_filter {
                tracing::error!(
                    "r2e: invalid tracing filter {:?} ({e}); falling back to \"info\"",
                    opts.filter
                );
            }
            Ok(())
        }
        Err(_) => Err(AlreadyInstalled {
            installed: INSTALLED.get().cloned(),
        }),
    }
}
