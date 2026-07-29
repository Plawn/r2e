//! Managed SQLx transactions: the commit/rollback lifecycle shared by both
//! transaction sources, and the generation a rotating-pool transaction reports.

#[path = "../support/mod.rs"]
mod support;

#[cfg(feature = "sqlite")]
mod sqlite;
