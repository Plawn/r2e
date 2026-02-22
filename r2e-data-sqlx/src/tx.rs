//! Transaction wrapper with automatic lifecycle management.
//!
//! Provides [`Tx`], [`HasPool`], and a blanket [`ManagedResource`] implementation
//! so that `#[managed]` handler parameters "just work" for database transactions.

use r2e_core::error::HttpError;
use r2e_core::managed::{ManagedError, ManagedResource};
use sqlx::{Database, Pool, Transaction};
use std::ops::{Deref, DerefMut};

/// Trait for application states that contain a database pool.
///
/// Implement this for your app state so that [`Tx`] can acquire transactions
/// via `#[managed]`:
///
/// ```ignore
/// impl HasPool<Sqlite> for MyState {
///     fn pool(&self) -> &Pool<Sqlite> {
///         &self.pool
///     }
/// }
/// ```
pub trait HasPool<DB: Database> {
    fn pool(&self) -> &Pool<DB>;
}

/// A wrapper around SQLx [`Transaction`] for use with `#[managed]`.
///
/// When used with `#[managed]`, the transaction is:
/// - Acquired (begun) before the handler executes
/// - Committed if the handler returns `Ok` (or a non-Result type)
/// - Rolled back (on drop) if the handler returns `Err` or panics
///
/// # Example
///
/// ```ignore
/// use r2e_data_sqlx::Tx;
/// use sqlx::Sqlite;
///
/// #[post("/")]
/// async fn create(
///     &self,
///     body: Json<CreateUser>,
///     #[managed] tx: &mut Tx<'_, Sqlite>,
/// ) -> Result<Json<User>, HttpError> {
///     sqlx::query("INSERT INTO users (name, email) VALUES (?, ?)")
///         .bind(&body.name)
///         .bind(&body.email)
///         .execute(tx.as_mut())
///         .await
///         .map_err(|e| HttpError::Internal(e.to_string()))?;
///     Ok(Json(user))
/// }
/// ```
pub struct Tx<'a, DB: Database>(pub Transaction<'a, DB>);

impl<'a, DB: Database> Deref for Tx<'a, DB> {
    type Target = Transaction<'a, DB>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a, DB: Database> DerefMut for Tx<'a, DB> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<'a, DB: Database> Tx<'a, DB> {
    /// Unwraps the `Tx` into the inner `Transaction`.
    pub fn into_inner(self) -> Transaction<'a, DB> {
        self.0
    }

    /// Returns a mutable reference to the underlying connection.
    pub fn as_mut(&mut self) -> &mut <DB as Database>::Connection {
        &mut *self.0
    }
}

/// `ManagedResource` implementation for `Tx` — handles transaction lifecycle.
///
/// - `acquire`: begins a new transaction from the pool
/// - `release(true)`: commits the transaction
/// - `release(false)`: drops the transaction (automatic rollback)
impl<S, DB> ManagedResource<S> for Tx<'static, DB>
where
    DB: Database,
    S: HasPool<DB> + Send + Sync,
{
    type Error = ManagedError;

    async fn acquire(state: &S) -> Result<Self, Self::Error> {
        let tx = state
            .pool()
            .begin()
            .await
            .map_err(|e| ManagedError(HttpError::Internal(e.to_string())))?;
        Ok(Tx(tx))
    }

    async fn release(self, success: bool) -> Result<(), Self::Error> {
        if success {
            self.into_inner()
                .commit()
                .await
                .map_err(|e| ManagedError(HttpError::Internal(e.to_string())))?;
        }
        // If !success, the transaction is dropped and automatically rolled back
        Ok(())
    }
}
