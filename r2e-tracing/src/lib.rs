//! Owner of R2E's process-global `tracing` subscriber.
//!
//! A `tracing` subscriber is a one-shot process global: the first install wins
//! and every later one is a no-op. This crate holds the knobs R2E installs it
//! with ([`TracingOptions`], [`LogFormat`], [`SpanEvents`]), the install itself
//! ([`try_install`]) and the record of what won ([`installed`]).
//!
//! It sits at the bottom of the R2E graph (no tokio, no `r2e-*` dependency).
//! `r2e-core` binds the YAML `tracing:` section (`TracingConfig`) to
//! [`TracingOptions`] and re-exports every item, so applications rarely name
//! this crate directly.

mod install;
mod noise;
mod options;

pub use install::{installed, try_install, AlreadyInstalled};
pub use noise::UpstreamNoise;
pub use options::{LogFormat, SpanEvents, TracingOptions};

/// The `tracing-subscriber` version R2E builds its stack with — re-exported so
/// [`TracingOptions::effective_span_events`]'s `FmtSpan` and custom stacks
/// line up with it.
pub use tracing_subscriber;
