//! `DataSourceHealth`: the `SELECT 1` readiness check a datasource contributes
//! to the `HealthRegistry` an `AdvancedHealth` plugin provides.

#[path = "../support/mod.rs"]
mod support;

#[cfg(feature = "sqlite")]
mod sqlite;
