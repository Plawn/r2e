//! Owner of R2E's process-global `tracing` subscriber.
//!
//! A `tracing` subscriber is a one-shot process global: the first install wins
//! and every later one is a no-op. This crate holds the knobs R2E installs it
//! with ([`TracingOptions`], [`LogFormat`], [`SpanEvents`]), the install itself
//! ([`try_install`]) and the record of what won ([`installed`]).
//!
//! Nobody else rebuilds the stack; integrations **contribute** to it:
//!
//! - [noise rules](suppress) — drop a known-benign upstream event
//!   ([`NoiseFilter`] applies them);
//! - [late layers](contribute_layer) — add a layer after the install
//!   (custom stacks host the slot with [`late_layers`]);
//! - [field enrichers](add_enricher) — add fields to every console line.
//!
//! It sits at the bottom of the R2E graph (no tokio, no `r2e-*` dependency).
//! `r2e-core` binds the YAML `tracing:` section (`TracingConfig`) to
//! [`TracingOptions`] and re-exports every item, so applications rarely name
//! this crate directly.

mod console;
mod enrich;
mod install;
mod late;
mod noise;
mod options;

#[doc(hidden)]
pub use console::console_layer_with_writer;
pub use enrich::{add_enricher, EnrichContext, FieldEnricher};
pub use install::{installed, try_install, AlreadyInstalled};
pub use late::{contribute_layer, late_layers, BoxedLayer, LateLayers, NotWired};
pub use noise::{is_suppressed, suppress, suppress_error, NoiseFilter, NoiseRule};
pub use options::{LogFormat, SpanEvents, TracingOptions};

/// The `tracing-subscriber` version R2E builds its stack with — re-exported so
/// [`TracingOptions::effective_span_events`]'s `FmtSpan` and custom stacks
/// line up with it.
pub use tracing_subscriber;
