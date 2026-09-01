//! Scaffolding for `llm/tenancy.md`.

use r2e::prelude::*;
use r2e::tenant::TenantId;
use serde::Serialize;

/// The app's tenant directory: slug → DSN.
#[derive(Clone, Default)]
pub struct Directory;

impl Directory {
    /// `Ok(None)` = unknown tenant (→ 404), `Err` = the directory is down (→ 503).
    pub async fn dsn(&self, _tenant: &TenantId) -> Result<Option<String>, sqlx::Error> {
        Ok(None)
    }
}

/// The already-constructed directory handed to `PoolDirectory`.
#[allow(non_upper_case_globals)]
pub const directory: Directory = Directory;

/// The row the per-tenant controller selects.
#[derive(Serialize, sqlx::FromRow)]
pub struct Order {
    pub id: i64,
    pub name: String,
}
