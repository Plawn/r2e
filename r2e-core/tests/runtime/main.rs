//! Runtime & serving surface: the `rt` task facade, SO_REUSEPORT sharded
//! serving, socket options, tracing subscriber configuration, and the
//! dev-reload partial rebuild.

#[cfg(feature = "dev-reload")]
mod dev_reload;
mod rt;
mod sharded;
mod tcp_nodelay;
mod tracing_config;
