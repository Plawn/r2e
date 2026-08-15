//! Managed Diesel transactions for R2E.
//!
//! The crate supports SQLite, PostgreSQL, and MySQL through the matching Cargo
//! features. Register a Diesel r2d2 pool as a bean and use [`Tx`] (or the more
//! explicit [`DieselTx`]) as a `#[managed]` route parameter.
//!
//! For rotating credentials, register a [`DbPool`] built from a live-config
//! URL and take a [`DbTx`] instead; the pool swaps its underlying connections
//! when the value changes without restarting the app.
//!
//! # Multi-tenant pools (feature `tenant`)
//!
//! With the `tenant` feature (facade: `tenant-diesel`), the same lifecycle runs
//! on **the requesting tenant's** pool: [`TenantPools<Conn>`] is the per-tenant
//! pool bean, [`PoolSource<Conn>`] builds each tenant's pool from a DSN lookup,
//! and [`TenantTx<Conn>`] is the managed transaction on it. [`TenantPool`]
//! carries the wiring, the compile-time guarantees, and the migrations deferral.

mod pool;
#[cfg(feature = "tenant")]
mod tenant;
mod tx;

pub use pool::{DbPool, PoolError};
#[cfg(feature = "tenant")]
pub use tenant::{PoolSource, TenantPool, TenantPools, TenantTx};
pub use tx::{DbTx, DieselTx, FixedPool, ManagedTx, RotatingPool, Tx, TxSource};

pub mod prelude {
    pub use crate::{DbPool, DbTx, DieselTx, Tx};
    #[cfg(feature = "tenant")]
    pub use crate::{TenantPools, TenantTx};
}
