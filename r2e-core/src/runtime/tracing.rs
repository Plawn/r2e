//! Core glue over [`r2e_tracing`]: installs the process-global subscriber from
//! the application's YAML `tracing:` section ([`TracingConfig`]).
//!
//! The subscriber stack itself (options, formats, the one-shot install, the
//! record of what won) lives in `r2e-tracing`; this module only binds
//! configuration to it and keeps the historical `r2e_core` entry points.

use crate::runtime::tracing_config::TracingConfig;

/// An init that lost the race to a subscriber installed earlier.
///
/// `installed` carries the winning configuration when R2E installed it, and
/// is `None` when the subscriber came from elsewhere (the application, a test
/// harness, another library).
#[derive(Debug, Clone)]
pub struct SubscriberAlreadyInstalled {
    /// The configuration that actually won, when it is known.
    pub installed: Option<TracingConfig>,
}

impl SubscriberAlreadyInstalled {
    /// Whether the winning subscriber differs from what `requested` asked
    /// for — i.e. whether losing the race actually changed the output.
    ///
    /// An unknown winner counts as different: nothing says it honours the
    /// requested format or filter.
    pub fn changes_output(&self, requested: &TracingConfig) -> bool {
        self.installed.as_ref() != Some(requested)
    }
}

impl From<r2e_tracing::AlreadyInstalled> for SubscriberAlreadyInstalled {
    fn from(lost: r2e_tracing::AlreadyInstalled) -> Self {
        Self {
            installed: lost.installed.as_ref().map(TracingConfig::from),
        }
    }
}

impl std::fmt::Display for SubscriberAlreadyInstalled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.installed {
            Some(c) => write!(
                f,
                "a global tracing subscriber is already installed (format={:?}, filter={:?})",
                c.effective_format(),
                c.filter
            ),
            None => write!(f, "a global tracing subscriber is already installed"),
        }
    }
}

impl std::error::Error for SubscriberAlreadyInstalled {}

/// Initialise the global `tracing` subscriber with a standard `fmt` layer.
///
/// Respects the `RUST_LOG` environment variable. Falls back to the
/// [`TracingConfig`] default filter (`info`) when `RUST_LOG` is not set.
///
/// This function is idempotent — calling it more than once is safe (subsequent
/// calls are silently ignored).
///
/// It installs the **built-in defaults**, ignoring the application's
/// `tracing:` section; R2E's entry points and the [`Tracing`] plugin call
/// [`init_tracing_from_config`] instead.
///
/// [`Tracing`]: crate::builtins::Tracing
pub fn init_tracing() {
    init_tracing_with_config(&TracingConfig::default());
}

/// Initialise the global `tracing` subscriber from the application's own
/// configuration — this is what R2E's entry points call.
///
/// Loads `application.yaml` (profile overlay and `R2E_*` env overlay
/// included) and installs its `tracing:` section, so an app that declares
/// `format: json` gets JSON from its very first log line, without having to
/// install a subscriber itself.
///
/// A missing configuration file is normal and silent. An unreadable or
/// malformed one falls back to the built-in defaults and warns — through the
/// subscriber it just installed, so the message is visible — rather than
/// failing: a bad config file must never cost the app the log line that
/// explains the boot error that follows.
pub fn init_tracing_from_config() {
    let (config, problem) = resolve_tracing_config();

    if let Err(lost) = try_init_tracing_with_config(&config) {
        warn_if_output_differs(&lost, &config);
    }
    if let Some(problem) = problem {
        tracing::warn!("r2e: falling back to the built-in tracing defaults — {problem}");
    }
}

/// Make sure a subscriber is installed, and return the options R2E installed
/// it with.
///
/// When R2E already installed one, its options are returned as is. Otherwise
/// this installs it from the application's `tracing:` section, exactly like
/// [`init_tracing_from_config`]. `None` means a subscriber R2E did not install
/// (the application's, a test harness's) holds the global slot.
///
/// Meant for plugins that contribute to the subscriber at setup time
/// ([`contribute_layer`](crate::contribute_layer),
/// [`add_enricher`](crate::add_enricher)): calling it first lets them work
/// under a hand-written `main` that never installed the subscriber.
pub fn ensure_tracing_installed() -> Option<&'static r2e_tracing::TracingOptions> {
    if let Some(installed) = r2e_tracing::installed() {
        return Some(installed);
    }
    init_tracing_from_config();
    r2e_tracing::installed()
}

/// The configuration [`init_tracing_from_config`] installs, plus the reason it
/// fell back to the built-in defaults when it did.
///
/// Split out of the install so the *resolution* — the part that reads files —
/// can be exercised on its own; installing is a one-shot process global and
/// can be observed only once per process.
#[doc(hidden)]
pub fn resolve_tracing_config() -> (TracingConfig, Option<String>) {
    match crate::config::R2eConfig::load() {
        Ok(loaded) => {
            use crate::config::ConfigProperties;
            match TracingConfig::from_config(&loaded, Some("tracing")) {
                Ok(config) => (config, None),
                Err(e) => (
                    TracingConfig::default(),
                    Some(format!("the `tracing:` section is invalid: {e}")),
                ),
            }
        }
        Err(e) => (
            TracingConfig::default(),
            Some(format!("the configuration could not be loaded: {e}")),
        ),
    }
}

/// Warn when an init lost the race to a subscriber that logs differently.
///
/// Losing with the very same configuration is the common, harmless case (an
/// entry point and a plugin reading the same section); staying quiet there is
/// what makes the warning worth reading when it does appear.
pub fn warn_if_output_differs(lost: &SubscriberAlreadyInstalled, requested: &TracingConfig) {
    if lost.changes_output(requested) {
        tracing::warn!(
            "r2e: this tracing configuration (format={:?}, filter={:?}) is ignored — {lost}. \
             Install the subscriber before R2E does (in `App::setup`), or opt out of R2E's with \
             `app_main!(MyApp, tracing = false)`.",
            requested.effective_format(),
            requested.filter,
        );
    }
}

/// Initialise the global `tracing` subscriber from a [`TracingConfig`].
///
/// `RUST_LOG` env var always takes priority over `config.filter`.
/// This function is idempotent — subsequent calls are silently ignored. Use
/// [`try_init_tracing_with_config`] to find out whether this call is the one
/// that took effect.
pub fn init_tracing_with_config(config: &TracingConfig) {
    let _ = try_init_tracing_with_config(config);
}

/// [`init_tracing_with_config`], reporting whether it won the race.
///
/// `Err(SubscriberAlreadyInstalled)` means a subscriber was already in place
/// and this configuration had no effect at all — the case that used to be a
/// silent no-op. The stack is built by [`r2e_tracing::try_install`].
pub fn try_init_tracing_with_config(
    config: &TracingConfig,
) -> Result<(), SubscriberAlreadyInstalled> {
    r2e_tracing::try_install(&config.into()).map_err(SubscriberAlreadyInstalled::from)
}
