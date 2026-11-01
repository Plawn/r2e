//! The one place R2E installs the process-global `tracing` subscriber:
//! [`AppBuilder::build_state`](crate::AppBuilder::build_state), first thing,
//! from the loaded `tracing:` section ([`TracingConfig`]) and the layers
//! contributed with `.tracing_layer(..)` on the builder and on
//! [`PluginSetupContext`](crate::plugin::PluginSetupContext).
//!
//! The stack itself (options, formats, the boot-log buffer, the one-shot
//! install) lives in `r2e-tracing`. The rest of this module is compile-time
//! migration shims for the removed entry points (see
//! `docs/migration/tracing-single-path.md`).

use crate::config::{ConfigProperties, R2eConfig};
use crate::runtime::tracing_config::TracingConfig;
use r2e_tracing::{AlreadyInstalled, BoxedLayer, TracingOptions};

/// Install the subscriber from `config`'s `tracing:` section plus `layers`.
///
/// Called by `try_build_state` before anything fallible, so a later boot
/// error is logged through the real subscriber. `config` is `None` when the
/// app never loaded one: the built-in defaults apply.
///
/// - `tracing.enabled: false` → nothing is installed and the boot-log buffer
///   is dropped (the application owns the subscriber, or wants none).
/// - Already installed by R2E (a second `TestApp::boot` in one process, a
///   dev-reload cycle after the first) → the contributed layers are ignored
///   (one DEBUG line).
/// - A foreign global subscriber won → one WARN when layers were contributed.
/// - Installed here → one INFO line with the effective options and where they
///   came from ([`install_summary`]), plus one WARN when no boot-log capture
///   was in place: whatever was logged before `build_state()` (a hand-written
///   `main` without `launch`) went nowhere.
pub(crate) fn install_tracing(config: Option<&R2eConfig>, layers: Vec<BoxedLayer>) {
    let (tracing, bind_error) = match config {
        Some(config) => match TracingConfig::from_config(config, Some("tracing")) {
            Ok(tracing) => (tracing, None),
            Err(e) => (TracingConfig::default(), Some(e.to_string())),
        },
        None => (TracingConfig::default(), None),
    };

    if !tracing.enabled {
        r2e_tracing::discard_boot_logs();
        return;
    }

    // Read before `install`, which puts the forwarder in place itself when
    // nobody did — and ends any capture.
    let captured = r2e_tracing::boot_logs_captured();
    let source = match config {
        Some(config) if bind_error.is_none() && config.has_prefix("tracing") => "config",
        _ => "defaults",
    };
    let contributed = layers.len();
    let options = TracingOptions::from(&tracing);
    match r2e_tracing::install(&options, layers) {
        Ok(()) => {
            tracing::info!("{}", install_summary(&options, contributed, source));
            if !captured {
                tracing::warn!("{NO_BOOT_CAPTURE}");
            }
        }
        Err(AlreadyInstalled { installed: Some(_) }) => {
            if contributed > 0 {
                tracing::debug!(
                    "r2e: the tracing subscriber is already installed; {contributed} \
                     contributed tracing layer(s) ignored"
                );
            }
        }
        Err(AlreadyInstalled { installed: None }) => {
            if contributed > 0 {
                tracing::warn!(
                    "r2e: {contributed} tracing layer(s) ignored: a subscriber was installed \
                     before the builder; set `tracing.enabled: false` to silence"
                );
            }
        }
    }

    if let Some(e) = bind_error {
        tracing::error!(
            "r2e: the `tracing:` section is invalid, using the built-in defaults — {e}"
        );
    }
}

/// The WARN `build_state()` logs when it installs the subscriber while no
/// boot-log capture was in place.
const NO_BOOT_CAPTURE: &str = "r2e: no boot-log capture was active: anything logged \
     before build_state() was dropped. Use r2e::launch (env reads and upstream waits belong \
     in App::setup), or call `let _boot = r2e::capture_boot_logs();` first in main — see \
     docs/migration/tracing-single-path.md";

/// The INFO line `build_state()` logs after installing the subscriber: the
/// effective options and their provenance.
///
/// `source` is `config` when the loaded configuration has at least one
/// `tracing.*` key (YAML or `R2E_TRACING_*`), `defaults` otherwise (no
/// configuration, no `tracing` key, or an invalid section). The filter is
/// the one the install applies: `RUST_LOG` when set and valid (marked
/// `(RUST_LOG)`), else `tracing.filter`, else `info` when that is invalid.
/// `ansi=auto:on|off` is the unset default resolved against stdout.
fn install_summary(options: &TracingOptions, contributed: usize, source: &str) -> String {
    use r2e_tracing::tracing_subscriber::EnvFilter;
    use r2e_tracing::{LogFormat, SpanEvents};
    use std::io::IsTerminal;

    let format = match options.effective_format() {
        LogFormat::Json if options.json.flatten => "json(flat)",
        LogFormat::Json => "json",
        LogFormat::Pretty => "pretty",
    };
    let filter = match std::env::var("RUST_LOG") {
        Ok(env) if EnvFilter::try_from_default_env().is_ok() => format!("{env:?} (RUST_LOG)"),
        _ if EnvFilter::try_new(&options.filter).is_err() => {
            format!("\"info\" (invalid {:?})", options.filter)
        }
        _ => format!("{:?}", options.filter),
    };
    let ansi = match options.ansi {
        Some(true) => "on",
        Some(false) => "off",
        None if std::io::stdout().is_terminal() => "auto:on",
        None => "auto:off",
    };
    let span_events = match options.span_events.clone().unwrap_or_default() {
        SpanEvents::None => "none",
        SpanEvents::New => "new",
        SpanEvents::Close => "close",
        SpanEvents::Active => "active",
        SpanEvents::Full => "full",
    };
    let mut line = format!(
        "r2e: tracing installed: format={format} filter={filter} ansi={ansi} \
         span_events={span_events}"
    );
    if !options.console {
        line.push_str(" console=off");
    }
    if contributed > 0 {
        line.push_str(&format!(" layers={contributed}"));
    }
    line.push_str(&format!(" source={source}"));
    line
}

// ── Removed entry points (migration diagnostics) ────────────────────────────

mod removed {
    /// Sealed: nothing outside this module can implement it, and nothing inside
    /// does — so every bound on it is unsatisfiable.
    pub trait Sealed {}
}

/// Marker for the **removed** `init_tracing*` / `ensure_tracing_installed`
/// entry points. Implemented by nothing: it turns a leftover call into a
/// compile error that names the migration page.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "`init_tracing*` was removed: `launch`/`TestApp::boot` capture boot logs and `build_state()` installs the subscriber; call `r2e_tracing::install(&TracingOptions::default(), vec![])` in a test without an app — see docs/migration/tracing-single-path.md",
    label = "removed tracing entry point",
    note = "the subscriber is configured by the `tracing:` section (`tracing.enabled`, `tracing.console`, `tracing.json`, …); contribute layers with `.tracing_layer(..)`"
)]
pub trait TracingEntryPointRemoved: removed::Sealed {}

/// Marker for the **removed** `Tracing` / `ConfiguredTracing` plugins.
/// Implemented by nothing — see [`TracingEntryPointRemoved`].
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "the `Tracing` / `ConfiguredTracing` plugins were removed: the subscriber is installed by `build_state()` from the `tracing:` section; contribute layers with `.tracing_layer(..)` — see docs/migration/tracing-single-path.md",
    label = "removed tracing plugin",
    note = "delete the `.plugin(Tracing…)` line and move its settings to the `tracing:` section of application.yaml"
)]
pub trait TracingPluginRemoved: removed::Sealed {}

/// **Removed.** See `docs/migration/tracing-single-path.md`.
#[doc(hidden)]
#[deprecated(
    note = "removed: `build_state()` installs the subscriber from the `tracing:` section — see docs/migration/tracing-single-path.md"
)]
pub fn init_tracing<'a>()
where
    &'a (): TracingEntryPointRemoved,
{
}

/// **Removed.** See `docs/migration/tracing-single-path.md`.
#[doc(hidden)]
#[deprecated(
    note = "removed: `build_state()` installs the subscriber from the `tracing:` section — see docs/migration/tracing-single-path.md"
)]
pub fn init_tracing_from_config<'a>()
where
    &'a (): TracingEntryPointRemoved,
{
}

/// **Removed.** See `docs/migration/tracing-single-path.md`.
#[doc(hidden)]
#[deprecated(
    note = "removed: `build_state()` installs the subscriber from the `tracing:` section — see docs/migration/tracing-single-path.md"
)]
pub fn init_tracing_with_config<'a>(_config: &TracingConfig)
where
    &'a (): TracingEntryPointRemoved,
{
}

/// **Removed.** See `docs/migration/tracing-single-path.md`.
#[doc(hidden)]
#[deprecated(
    note = "removed: `build_state()` installs the subscriber from the `tracing:` section — see docs/migration/tracing-single-path.md"
)]
pub fn try_init_tracing_with_config<'a>(_config: &TracingConfig) -> Result<(), AlreadyInstalled>
where
    &'a (): TracingEntryPointRemoved,
{
    Ok(())
}

/// **Removed.** See `docs/migration/tracing-single-path.md`.
#[doc(hidden)]
#[deprecated(
    note = "removed: contribute layers with `PluginSetupContext::tracing_layer(..)`; `build_state()` installs the subscriber — see docs/migration/tracing-single-path.md"
)]
pub fn ensure_tracing_installed<'a>() -> Option<&'static TracingOptions>
where
    &'a (): TracingEntryPointRemoved,
{
    None
}
