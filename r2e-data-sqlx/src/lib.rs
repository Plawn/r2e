//! # r2e-data-sqlx — SQLx backend for R2E data layer
//!
//! This crate provides the [SQLx](https://github.com/launchbadge/sqlx)-specific
//! implementations for R2E's data access layer. It depends on [`r2e-data`] for
//! the abstract traits and types, and adds the repository wrapper, transaction
//! utilities, and error bridging needed to talk to a real database.
//!
//! # What's in this crate
//!
//! | Type | Description |
//! |------|-------------|
//! | [`SqlxRepository`] | Generic repository wrapper holding an `sqlx::Pool<DB>` |
//! | [`Tx`] | Transaction wrapper for use with `#[managed]` — auto-commit / auto-rollback |
//! | [`HasPool`] | Trait for application states that contain a database pool |
//! | [`SqlxErrorExt`] | Extension trait to convert `sqlx::Error` → `DataError` (`.into_data_error()`) |
//! | [`SqlxResult<T>`] | Type alias for `Result<T, DataError>` |
//! | [`migration`] | Documentation module with guidance on using `sqlx::migrate!()` |
//!
//! # Feature flags
//!
//! Enable exactly one database driver:
//!
//! | Feature    | Driver |
//! |------------|--------|
//! | `sqlite`   | SQLite via `sqlx/sqlite` |
//! | `postgres` | PostgreSQL via `sqlx/postgres` |
//! | `mysql`    | MySQL via `sqlx/mysql` |
//!
//! # Quick start
//!
//! ```toml
//! [dependencies]
//! r2e-data-sqlx = { version = "0.1", features = ["sqlite"] }
//! ```
//!
//! ```ignore
//! use r2e_data_sqlx::{SqlxRepository, Tx, HasPool};
//! use sqlx::Sqlite;
//!
//! // Use SqlxRepository as a convenience pool wrapper
//! let repo = SqlxRepository::<UserEntity, Sqlite>::new(pool.clone());
//!
//! // Use Tx with #[managed] for automatic transaction lifecycle
//! #[post("/")]
//! async fn create(
//!     &self,
//!     body: Json<CreateUser>,
//!     #[managed] tx: &mut Tx<'_, Sqlite>,
//! ) -> Result<Json<User>, AppError> {
//!     sqlx::query("INSERT INTO users (name) VALUES (?)")
//!         .bind(&body.name)
//!         .execute(tx.as_mut())
//!         .await?;
//!     Ok(Json(user))
//! }
//! ```
//!
//! # Transaction management
//!
//! The [`Tx`] type implements `ManagedResource` and is designed for use with
//! R2E's `#[managed]` attribute:
//!
//! - **Acquire:** begins a new transaction from the pool
//! - **Release (success):** commits the transaction
//! - **Release (failure):** drops the transaction (automatic rollback)
//!
//! Your application state must implement [`HasPool<DB>`] for the database type
//! you're using:
//!
//! ```ignore
//! impl HasPool<Sqlite> for MyState {
//!     fn pool(&self) -> &Pool<Sqlite> {
//!         &self.pool
//!     }
//! }
//! ```
//!
//! # Error bridging
//!
//! Due to Rust's orphan rules, `From<sqlx::Error> for DataError` can't be
//! implemented here. Use the [`SqlxErrorExt`] trait instead:
//!
//! ```ignore
//! use r2e_data_sqlx::SqlxErrorExt;
//!
//! let user = sqlx::query_as("SELECT ...")
//!     .fetch_one(&pool)
//!     .await
//!     .map_err(|e| e.into_data_error())?;
//! ```

pub mod error;
pub mod migration;
pub mod repository;
pub mod tx;

pub use error::{SqlxErrorExt, SqlxResult};
pub use repository::SqlxRepository;
pub use tx::{HasPool, Tx};

/// Re-exports of the most commonly used types from both `r2e-data` and this crate.
pub mod prelude {
    pub use crate::{HasPool, SqlxErrorExt, SqlxRepository, Tx};
    pub use r2e_data::prelude::*;
}
