//! Runtime & serving surface: the `rt` task facade, SO_REUSEPORT sharded
//! serving, socket options, and tracing subscriber configuration.
//!
//! The dev-reload hot-patch tests live in their own target
//! (`tests/dev_reload/`): they arm process-global, one-way state that would
//! make every serving test here skip its startup lifecycle. See
//! `tests/dev_reload/main.rs`.

mod rt;
mod sharded;
mod shutdown_budget;
mod tcp_nodelay;
mod tracing_config;
mod worker_scopes;
mod worker_services;
#[cfg(feature = "ws")]
mod ws_shutdown;
