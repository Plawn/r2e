//! Owner of R2E's process-global `tracing` subscriber.
//!
//! A `tracing` subscriber is a one-shot process global: the first install wins
//! and every later one is a no-op. R2E installs it **once**, from the
//! application builder, with [`install`]: the knobs of the `tracing:` section
//! ([`TracingOptions`], [`LogFormat`], [`SpanEvents`], [`JsonOptions`]) plus the
//! [`Layer`](tracing_subscriber::Layer)s plugins and the application
//! contributed — an OpenTelemetry layer, a [`Suppress`] rule for a known-benign
//! upstream event, a layer writing [`LogFields`] that the console prints on
//! every line. [`installed`] records what won.
//!
//! Logs emitted before that install (config loading, `App::setup`, plugin
//! setup) are not lost: [`capture_boot_logs`] puts a forwarding subscriber in
//! place first, which buffers them and replays them through the real stack
//! when [`install`] runs — or writes them to stderr when the boot fails before
//! (see [`BootLogs`]).
//!
//! It sits at the bottom of the R2E graph (no tokio, no `r2e-*` dependency).
//! `r2e-core` binds the YAML `tracing:` section to [`TracingOptions`] and calls
//! [`install`] from the builder, so applications rarely name this crate
//! directly.

mod boot;
mod console;
mod enrich;
mod flat_json;
mod install;
mod noise;
mod options;

#[doc(hidden)]
pub use boot::flush_boot_logs_to;
pub use boot::{capture_boot_logs, discard_boot_logs, BootLogs};
#[doc(hidden)]
pub use console::console_layer_with_writer;
pub use enrich::LogFields;
pub use install::{install, installed, AlreadyInstalled};
pub use noise::Suppress;
pub use options::{JsonOptions, LogFormat, SpanEvents, TracingOptions};

/// A type-erased layer over the [`Registry`](tracing_subscriber::Registry) —
/// what [`install`] takes from plugins and the application.
pub type BoxedLayer =
    Box<dyn tracing_subscriber::Layer<tracing_subscriber::Registry> + Send + Sync + 'static>;

/// The `tracing-subscriber` version R2E builds its stack with — re-exported so
/// [`TracingOptions::effective_span_events`]'s `FmtSpan` and contributed
/// layers line up with it.
pub use tracing_subscriber;
