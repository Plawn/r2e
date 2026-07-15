//! Managed Diesel transactions for R2E.
//!
//! The crate supports SQLite, PostgreSQL, and MySQL through the matching Cargo
//! features. Register a Diesel r2d2 pool as a bean and use [`Tx`] (or the more
//! explicit [`DieselTx`]) as a `#[managed]` route parameter.

use diesel::{
    connection::TransactionManager,
    r2d2::{ConnectionManager, Pool, PooledConnection, R2D2Connection},
    Connection,
};
use r2e_core::{
    BeanLookup, HttpError, ManagedContext, ManagedErr, ManagedOutcome, ManagedResource,
};
use std::ops::{Deref, DerefMut};

/// Request-scoped Diesel transaction backed by an r2d2 pooled connection.
pub struct DieselTx<Conn>
where
    Conn: Connection + R2D2Connection + 'static,
{
    connection: Option<PooledConnection<ConnectionManager<Conn>>>,
}

/// Short name for applications depending directly on this backend crate.
pub type Tx<Conn> = DieselTx<Conn>;

impl<Conn> DieselTx<Conn>
where
    Conn: Connection + R2D2Connection + Send + 'static,
{
    /// Direct access for code already executing on a blocking thread.
    /// Prefer [`Self::run`] from async route handlers.
    pub fn connection(&mut self) -> &mut Conn {
        &mut *self
            .connection
            .as_mut()
            .expect("managed Diesel transaction has already been finalized")
    }

    /// Executes one Diesel operation on Tokio's blocking pool while retaining
    /// the same connection and transaction for subsequent calls.
    pub async fn run<F, T>(&mut self, operation: F) -> Result<T, HttpError>
    where
        F: FnOnce(&mut Conn) -> diesel::QueryResult<T> + Send + 'static,
        T: Send + 'static,
    {
        let mut connection = self.connection.take().ok_or_else(|| {
            HttpError::internal("managed Diesel transaction has already been finalized")
        })?;
        let joined = tokio::task::spawn_blocking(move || {
            let result = operation(&mut connection);
            (connection, result)
        })
        .await
        .map_err(|error| HttpError::internal(format!("Diesel task failed: {error}")))?;
        self.connection = Some(joined.0);
        joined
            .1
            .map_err(|error| HttpError::internal(error.to_string()))
    }
}

impl<Conn> Deref for DieselTx<Conn>
where
    Conn: Connection + R2D2Connection + 'static,
{
    type Target = Conn;

    fn deref(&self) -> &Self::Target {
        &*self
            .connection
            .as_ref()
            .expect("managed Diesel transaction has already been finalized")
    }
}

impl<Conn> DerefMut for DieselTx<Conn>
where
    Conn: Connection + R2D2Connection + 'static,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut *self
            .connection
            .as_mut()
            .expect("managed Diesel transaction has already been finalized")
    }
}

impl<S, Conn> ManagedResource<S> for DieselTx<Conn>
where
    S: BeanLookup + Send + Sync,
    Conn: Connection + R2D2Connection + Send + 'static,
{
    type Error = ManagedErr<HttpError>;

    async fn acquire(context: ManagedContext<'_, S>) -> Result<Self, Self::Error> {
        let pool = context
            .state
            .bean::<Pool<ConnectionManager<Conn>>>()
            .ok_or_else(|| {
                ManagedErr(HttpError::internal(format!(
                    "database pool bean `{}` not found for {}::{}; call .provide(pool) before build_state()",
                    std::any::type_name::<Pool<ConnectionManager<Conn>>>(),
                    context.controller,
                    context.handler,
                )))
            })?;
        let connection = tokio::task::spawn_blocking(move || {
            let mut connection = pool.get().map_err(|error| error.to_string())?;
            <Conn::TransactionManager as TransactionManager<Conn>>::begin_transaction(
                &mut connection,
            )
            .map_err(|error| error.to_string())?;
            Ok::<_, String>(connection)
        })
        .await
        .map_err(|error| ManagedErr(HttpError::internal(format!("Diesel task failed: {error}"))))?
        .map_err(|error| ManagedErr(HttpError::internal(error)))?;
        Ok(Self {
            connection: Some(connection),
        })
    }

    async fn finalize(&mut self, outcome: &ManagedOutcome) -> Result<(), Self::Error> {
        let Some(mut connection) = self.connection.take() else {
            return Ok(());
        };
        let success = outcome.is_success();
        tokio::task::spawn_blocking(move || {
            if success {
                <Conn::TransactionManager as TransactionManager<Conn>>::commit_transaction(
                    &mut connection,
                )
            } else {
                <Conn::TransactionManager as TransactionManager<Conn>>::rollback_transaction(
                    &mut connection,
                )
            }
        })
        .await
        .map_err(|error| ManagedErr(HttpError::internal(format!("Diesel task failed: {error}"))))?
        .map_err(|error| ManagedErr(HttpError::internal(error.to_string())))
    }

    fn abort(&mut self) {
        // An r2d2 Diesel connection with an open transaction is considered
        // broken and discarded instead of being returned to the pool.
        drop(self.connection.take());
    }
}

pub mod prelude {
    pub use crate::DieselTx;
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use super::*;
    use diesel::{sql_query, RunQueryDsl, SqliteConnection};
    use r2e_core::{AppBuilder, ManagedGuard};

    fn pool_with_table() -> Pool<ConnectionManager<SqliteConnection>> {
        let manager = ConnectionManager::<SqliteConnection>::new(":memory:");
        let pool = Pool::builder().max_size(1).build(manager).unwrap();
        sql_query("CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT NOT NULL)")
            .execute(&mut pool.get().unwrap())
            .unwrap();
        pool
    }

    #[tokio::test]
    async fn commits_and_rolls_back_from_http_outcome() {
        let app = AppBuilder::new()
            .provide(pool_with_table())
            .build_state()
            .await;
        let mut committed = ManagedGuard::<DieselTx<SqliteConnection>, _>::acquire(
            ManagedContext::new(app.state(), "Test", "commit"),
        )
        .await
        .unwrap();
        committed
            .resource_mut()
            .run(|connection| {
                sql_query("INSERT INTO items(name) VALUES ('committed')").execute(connection)
            })
            .await
            .unwrap();
        committed
            .finalize(&ManagedOutcome::from_status(
                r2e_core::http::StatusCode::CREATED,
            ))
            .await
            .unwrap();

        let mut rolled_back = ManagedGuard::<DieselTx<SqliteConnection>, _>::acquire(
            ManagedContext::new(app.state(), "Test", "rollback"),
        )
        .await
        .unwrap();
        rolled_back
            .resource_mut()
            .run(|connection| {
                sql_query("INSERT INTO items(name) VALUES ('rolled back')").execute(connection)
            })
            .await
            .unwrap();
        rolled_back
            .finalize(&ManagedOutcome::from_status(
                r2e_core::http::StatusCode::BAD_REQUEST,
            ))
            .await
            .unwrap();

        let pool = app
            .state()
            .bean::<Pool<ConnectionManager<SqliteConnection>>>()
            .unwrap();
        let mut connection = pool.get().unwrap();
        #[derive(diesel::QueryableByName)]
        struct Count {
            #[diesel(sql_type = diesel::sql_types::BigInt)]
            count: i64,
        }
        let count = sql_query("SELECT COUNT(*) AS count FROM items")
            .get_result::<Count>(&mut connection)
            .unwrap();
        assert_eq!(count.count, 1);
    }
}
