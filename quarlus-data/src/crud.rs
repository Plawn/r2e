use crate::entity::Entity;
use crate::query::QueryBuilder;
use sqlx::{Database, Pool};
use std::marker::PhantomData;

/// A generic SQL-based repository implementation.
///
/// Uses the `Entity` trait to construct SQL queries dynamically.
/// Requires that the entity type also implements `sqlx::FromRow`.
///
/// # Example
///
/// ```ignore
/// let repo = SqlxRepository::<UserEntity, Sqlite>::new(pool.clone());
/// let users = repo.find_all().await?;
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

    /// Create a `QueryBuilder` pre-configured for this entity's table.
    pub fn query(&self) -> QueryBuilder
    where
        T: Entity,
    {
        QueryBuilder::new(T::table_name())
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
