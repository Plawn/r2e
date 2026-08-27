//! The `SqlxDataSource` plugin: config-driven connection, migrations at boot,
//! named datasources, and the pinned-pool escape hatch.

#[path = "../support/mod.rs"]
mod support;

#[cfg(feature = "sqlite")]
mod module;
#[cfg(feature = "sqlite")]
mod sqlite;
