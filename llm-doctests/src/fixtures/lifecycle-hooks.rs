//! Scaffolding for `llm/lifecycle-hooks.md`.

use r2e::prelude::*;

/// Not in the prelude: `use std::sync::Arc;` / `use sqlx::SqlitePool;`.
pub use sqlx::SqlitePool;
pub use std::sync::Arc;

/// The event the controller's `#[consumer]` reads.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Ping;

/// The plain config value the `#[pre_destroy]` pool takes.
#[derive(Clone)]
pub struct PoolConfig;

/// The collaborator the `#[on_start]` bean injects.
#[derive(Clone)]
pub struct Store;

#[bean]
impl Store {
    pub fn new() -> Self {
        Self
    }

    /// The slow work `WarmCache` preloads at boot.
    pub async fn load(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
}
