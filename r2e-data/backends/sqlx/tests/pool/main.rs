//! The rotating `DbPool` facade: live-config driven rotation, snapshot
//! coherence, and the closed-pool retry window a rotation opens.

#[path = "../support/mod.rs"]
mod support;

#[cfg(feature = "sqlite")]
mod sqlite;
