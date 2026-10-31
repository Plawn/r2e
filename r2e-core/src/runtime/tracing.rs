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

    let contributed = layers.len();
    match r2e_tracing::install(&TracingOptions::from(&tracing), layers) {
        Ok(()) => {}
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
