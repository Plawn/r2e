//! Managed SQLx transactions for R2E.
//!
//! Install the datasource plugin — it connects the pool, optionally migrates,
//! and closes it on shutdown — then request a [`DbTx`] from an HTTP route:
//!
//! ```ignore
//! AppBuilder::new()
//!     .load_config::<()>()
//!     .plugin(SqlxDataSource::<sqlx::Postgres>::new().migrations(&MIGRATOR))
//!     .build_state()
//!     .await;
//!
//! #[post("/users")]
//! async fn create(
//!     &self,
//!     #[managed] tx: &mut r2e_data_sqlx::Tx<'_, sqlx::Postgres>,
//! ) -> Result<StatusCode, HttpError> {
//!     sqlx::query("INSERT INTO users(name) VALUES ($1)")
//!         .bind("Ada")
//!         .execute(tx.connection())
//!         .await
//!         .map_err(|error| HttpError::internal(error.to_string()))?;
//!     Ok(StatusCode::CREATED)
//! }
//! ```
//!
//! Responses below status 400 commit. Client/server error responses roll back.
//! Cancellation and panic use SQLx's drop rollback as a safety fallback.
//!
//! # Multi-tenant pools (feature `tenant`)
//!
//! With the `tenant` feature (facade: `tenant-sqlx`), the same lifecycle runs on
//! **the requesting tenant's** pool: `TenantPools<DB>` is the per-tenant pool
//! bean, `PoolSource<DB>` builds each tenant's pool from a DSN lookup, and
//! `TenantTx<'_, DB>` is the managed transaction on it. `TenantPool` carries the
//! wiring, the compile-time guarantees, and the migrations deferral.

mod datasource;
mod health;
mod pool;
#[cfg(feature = "tenant")]
mod tenant;
mod tx;

pub use datasource::{DataSourceConfig, DataSourceTag, DefaultDataSource, SqlxDataSource};
pub use health::DataSourceHealth;
pub use pool::DbPool;
#[cfg(feature = "tenant")]
pub use tenant::{PoolSource, TenantPool, TenantPools, TenantTx};
pub use tx::{DbTx, FixedPool, ManagedTx, RotatingPool, SqlxTx, Tx, TxSource};

pub mod prelude {
    pub use crate::{DataSourceHealth, DbPool, DbTx, SqlxDataSource, SqlxTx, Tx};
    #[cfg(feature = "tenant")]
    pub use crate::{TenantPools, TenantTx};
}
