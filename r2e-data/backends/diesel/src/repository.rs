use diesel::r2d2::{ConnectionManager, Pool, R2D2Connection};
use std::marker::PhantomData;

/// A generic Diesel-based repository wrapper.
///
/// Holds an `r2d2::Pool` and provides access to pooled connections.
///
/// # Example
///
/// ```ignore
/// use diesel::sqlite::SqliteConnection;
///
/// let pool = Pool::builder()
///     .build(ConnectionManager::<SqliteConnection>::new("db.sqlite"))
///     .expect("Failed to create pool");
///
/// let repo = DieselRepository::<UserEntity, SqliteConnection>::new(pool);
/// ```
pub struct DieselRepository<T, Conn: diesel::Connection + R2D2Connection + 'static> {
    pool: Pool<ConnectionManager<Conn>>,
    _marker: PhantomData<T>,
}

impl<T, Conn: diesel::Connection + R2D2Connection + 'static> DieselRepository<T, Conn> {
    pub fn new(pool: Pool<ConnectionManager<Conn>>) -> Self {
        Self {
            pool,
            _marker: PhantomData,
        }
    }

    /// Get a reference to the underlying connection pool.
    pub fn pool(&self) -> &Pool<ConnectionManager<Conn>> {
        &self.pool
    }
}

impl<T, Conn: diesel::Connection + R2D2Connection + 'static> Clone for DieselRepository<T, Conn> {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
            _marker: PhantomData,
        }
    }
}
