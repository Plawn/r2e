//! The rotating `DbPool` facade: live-config driven rotation, snapshot
//! coherence, and the disposal semantics of the pool a rotation replaces.

#[path = "../support/mod.rs"]
mod support;

#[cfg(feature = "sqlite")]
mod sqlite;
