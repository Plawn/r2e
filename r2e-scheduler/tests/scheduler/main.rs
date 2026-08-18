//! Scheduler subsystem tests: one target, one module per source concern.
//!
//! Driver edge cases (min-heap ordering, drift, drain races) live in the
//! sibling `driver_edge_test` target.

mod core;
mod duration;
mod dynamic;
mod handle;
mod handle_sync;
mod overlap;
mod plugin;
mod plugin_config;
mod runtime_control;
mod serve_lifecycle;
#[cfg(all(
    unix,
    not(any(target_os = "solaris", target_os = "illumos", target_os = "cygwin"))
))]
mod sharded;
mod skip_if;
mod types;
