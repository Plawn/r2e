//! Managed SQLx transactions for R2E.
//!
//! Register an SQLx pool as a bean, then request a [`Tx`] from an HTTP route:
//!
//! ```ignore
//! AppBuilder::new().provide(pool).build_state().await;
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

mod tx;

pub use tx::{SqlxTx, Tx};

pub mod prelude {
    pub use crate::{SqlxTx, Tx};
}
