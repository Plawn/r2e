use sqlx::{Database, Pool};
use std::marker::PhantomData;

/// A generic SQL-based repository implementation.
///
/// Wraps an `sqlx::Pool<DB>` for a given entity type.
///
/// # Example
///
/// ```ignore
/// let repo = SqlxRepository::<UserEntity, Sqlite>::new(pool.clone());
/// ```
pub struct SqlxRepository<T, DB: Database> {
    pool: Pool<DB>,
    _marker: PhantomData<T>,
}

impl<T, DB: Database> SqlxRepository<T, DB> {
    pub fn new(pool: Pool<DB>) -> Self {
        Self {
            pool,
            _marker: PhantomData,
        }
    }

    /// Get the underlying pool reference.
    pub fn pool(&self) -> &Pool<DB> {
        &self.pool
    }
}

impl<T, DB: Database> Clone for SqlxRepository<T, DB> {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
            _marker: PhantomData,
        }
    }
}
