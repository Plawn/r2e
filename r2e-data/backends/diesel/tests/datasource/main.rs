//! The `DieselDataSource` plugin: config-driven pool, migrations at boot,
//! named datasources, and the pinned-pool escape hatch.

#[path = "../support/mod.rs"]
mod support;

#[cfg(feature = "sqlite")]
mod sqlite;
