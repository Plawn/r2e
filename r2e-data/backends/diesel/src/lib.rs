//! Managed Diesel transactions for R2E.
//!
//! The crate supports SQLite, PostgreSQL, and MySQL through the matching Cargo
//! features. Register a Diesel r2d2 pool as a bean and use [`Tx`] (or the more
//! explicit [`DieselTx`]) as a `#[managed]` route parameter.
//!
//! For rotating credentials, register a [`DbPool`] built from a live-config
//! URL and take a [`DbTx`] instead; the pool swaps its underlying connections
//! when the value changes without restarting the app.

mod pool;
mod tx;

pub use pool::{DbPool, PoolError};
pub use tx::{DbTx, DieselTx, FixedPool, ManagedTx, RotatingPool, Tx, TxSource};

pub mod prelude {
    pub use crate::{DbPool, DbTx, DieselTx, Tx};
}
