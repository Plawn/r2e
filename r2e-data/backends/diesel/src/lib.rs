//! Managed Diesel transactions for R2E.
//!
//! The crate supports SQLite, PostgreSQL, and MySQL through the matching Cargo
//! features. Register a Diesel r2d2 pool as a bean and use [`Tx`] (or the more
//! explicit [`DieselTx`]) as a `#[managed]` route parameter.
//!
//! For rotating credentials, install the [`DieselDataSource`] plugin — it
//! builds a [`DbPool`] from the live-config `datasource.url`, optionally runs
//! the embedded migrations at boot, and swaps the underlying connections when
//! the value changes without restarting the app — and take a [`DbTx`].
//!
//! # Multi-tenant pools (feature `tenant`)
//!
//! With the `tenant` feature (facade: `tenant-diesel`), the same lifecycle runs
//! on **the requesting tenant's** pool: [`TenantPools<Conn>`] is the per-tenant
//! pool bean, [`PoolSource<Conn>`] builds each tenant's pool from a DSN lookup,
//! and [`TenantTx<Conn>`] is the managed transaction on it. [`TenantPool`]
//! carries the wiring, the compile-time guarantees, and the migrations deferral.

mod datasource;
mod health;
mod pool;
#[cfg(feature = "tenant")]
mod tenant;
mod tx;

pub use datasource::{DataSourceConfig, DataSourceTag, DefaultDataSource, DieselDataSource};
pub use health::DataSourceHealth;
pub use pool::{DbPool, PoolError, PoolFactory};
#[cfg(feature = "tenant")]
pub use tenant::{PoolSource, TenantPool, TenantPools, TenantTx};
pub use tx::{DbTx, DieselTx, FixedPool, ManagedTx, RotatingPool, Tx, TxSource};

pub mod prelude {
    pub use crate::{DataSourceHealth, DbPool, DbTx, DieselDataSource, DieselTx, Tx};
    #[cfg(feature = "tenant")]
    pub use crate::{TenantPools, TenantTx};
}
