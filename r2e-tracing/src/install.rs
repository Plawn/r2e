use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{EnvFilter, Registry};

use crate::boot;
use crate::console::console_layer;
use crate::options::TracingOptions;
use crate::BoxedLayer;

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

/// Serialises installs: the loser of two concurrent ones must see the
/// winner's options.
static INSTALL: Mutex<()> = Mutex::new(());

/// The options the process-global subscriber was installed with, when
/// [`install`] installed it; `None` when nothing is installed yet or the
/// subscriber came from elsewhere (the application, a test harness, another
/// library).
pub fn installed() -> Option<&'static TracingOptions> {
    INSTALLED.get()
}

/// An install that lost the race to a subscriber installed earlier.
///
/// `installed` carries the winning options when R2E installed it, and is
/// `None` when the subscriber came from elsewhere (a global subscriber was set
/// before [`capture_boot_logs`](crate::capture_boot_logs) could put R2E's in
/// place).
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

/// Install the process-global `tracing` subscriber built from `opts` and the
/// contributed `layers`.
///
/// The stack is one plain `Layered` value:
///
/// ```text
/// Registry
///   + [layers…, console]   // contributed layers in order, then the console layer
///                          // (text, JSON, or flat JSON, printing LogFields;
///                          // left out when `opts.console` is false)
///   + EnvFilter            // outer, global
/// ```
///
/// The vector forwards `event_enabled` conjunctively, so a
/// [`Suppress`](crate::Suppress) among `layers` gates the console and every
/// sibling. `RUST_LOG`, when set and valid, takes priority over `opts.filter`.
/// An invalid `opts.filter` does not fail the install: the subscriber falls
/// back to `"info"` and logs an ERROR naming the directive. With `opts.ansi`
/// unset, colours are on only when stdout is a terminal.
///
/// The stack is handed to the boot forwarder: the logs buffered since
/// [`capture_boot_logs`](crate::capture_boot_logs) are replayed through it,
/// then every event goes to it. When nobody captured boot logs, `install`
/// does it first. It also works after
/// [`discard_boot_logs`](crate::discard_boot_logs) (logging turns on from
/// here, nothing is replayed).
///
/// `Err(AlreadyInstalled)` means these options and layers had no effect at
/// all: R2E already installed a subscriber (`installed: Some(..)`), or another
/// global subscriber was set first (`installed: None`). On success the
/// options are recorded and [`installed`] returns them.
pub fn install(opts: &TracingOptions, layers: Vec<BoxedLayer>) -> Result<(), AlreadyInstalled> {
    let _serial = INSTALL.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(won) = INSTALLED.get() {
        return Err(AlreadyInstalled {
            installed: Some(won.clone()),
        });
    }
    if !boot::ensure_forwarder() {
        return Err(AlreadyInstalled { installed: None });
    }

    let mut invalid_filter = None;
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::try_new(&opts.filter).unwrap_or_else(|e| {
            invalid_filter = Some(e);
            EnvFilter::new("info")
        })
    });

    let mut layers = layers;
    if opts.console {
        layers.push(console_layer(opts));
    }
    let stack = Registry::default().with(layers).with(env_filter);

    if !boot::take_over(Arc::new(stack)) {
        // Unreachable under `INSTALL` unless a previous install panicked
        // half-way; report it as installed by someone else.
        return Err(AlreadyInstalled { installed: None });
    }
    let _ = INSTALLED.set(opts.clone());
    if let Some(e) = invalid_filter {
        tracing::error!(
            "r2e: invalid tracing filter {:?} ({e}); falling back to \"info\"",
            opts.filter
        );
    }
    Ok(())
}
