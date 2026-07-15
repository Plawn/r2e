use r2e_core::{
    BeanLookup, HttpError, ManagedContext, ManagedErr, ManagedOutcome, ManagedResource,
};
use sqlx::{Database, Pool, Transaction};
use std::ops::{Deref, DerefMut};

/// Request-scoped SQLx transaction managed by R2E.
///
/// A `Pool<DB>` must be registered as a bean. Successful HTTP responses
/// (status below 400) commit; error responses roll back explicitly. Dropping
/// an unfinished transaction provides SQLx's rollback fallback.
pub struct SqlxTx<'a, DB: Database> {
    inner: Option<Transaction<'a, DB>>,
}

/// Backward-compatible short name used in handler signatures.
pub type Tx<'a, DB> = SqlxTx<'a, DB>;

impl<'a, DB: Database> SqlxTx<'a, DB> {
    pub fn connection(&mut self) -> &mut DB::Connection {
        self.as_mut()
    }

    pub fn as_mut(&mut self) -> &mut DB::Connection {
        &mut *self
            .inner
            .as_mut()
            .expect("managed SQLx transaction has already been finalized")
    }
}

impl<'a, DB: Database> Deref for SqlxTx<'a, DB> {
    type Target = Transaction<'a, DB>;

    fn deref(&self) -> &Self::Target {
        self.inner
            .as_ref()
            .expect("managed SQLx transaction has already been finalized")
    }
}

impl<'a, DB: Database> DerefMut for SqlxTx<'a, DB> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner
            .as_mut()
            .expect("managed SQLx transaction has already been finalized")
    }
}

impl<S, DB> ManagedResource<S> for SqlxTx<'static, DB>
where
    DB: Database,
    S: BeanLookup + Send + Sync,
{
    type Error = ManagedErr<HttpError>;

    async fn acquire(context: ManagedContext<'_, S>) -> Result<Self, Self::Error> {
        let pool = context.state.bean::<Pool<DB>>().ok_or_else(|| {
            ManagedErr(HttpError::internal(format!(
                "database pool bean `{}` not found for {}::{}; call .provide(pool) before build_state()",
                std::any::type_name::<Pool<DB>>(),
                context.controller,
                context.handler,
            )))
        })?;
        let transaction = pool
            .begin()
            .await
            .map_err(|error| ManagedErr(HttpError::internal(error.to_string())))?;
        Ok(Self {
            inner: Some(transaction),
        })
    }

    async fn finalize(&mut self, outcome: &ManagedOutcome) -> Result<(), Self::Error> {
        let Some(transaction) = self.inner.take() else {
            return Ok(());
        };
        let result = if outcome.is_success() {
            transaction.commit().await
        } else {
            transaction.rollback().await
        };
        result.map_err(|error| ManagedErr(HttpError::internal(error.to_string())))
    }

    fn abort(&mut self) {
        // SQLx rolls back an unfinished transaction when it is dropped.
        drop(self.inner.take());
    }
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use super::*;
    use r2e_core::{AppBuilder, ManagedGuard};
    use sqlx::{sqlite::SqlitePoolOptions, Row, Sqlite, SqlitePool};

    async fn pool_with_table() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    #[tokio::test]
    async fn commits_successful_outcome() {
        let app = AppBuilder::new()
            .provide(pool_with_table().await)
            .build_state()
            .await;
        let mut tx = ManagedGuard::<SqlxTx<'static, Sqlite>, _>::acquire(ManagedContext::new(
            app.state(),
            "Test",
            "commit",
        ))
        .await
        .unwrap();
        sqlx::query("INSERT INTO items(name) VALUES ('committed')")
            .execute(tx.resource_mut().connection())
            .await
            .unwrap();
        tx.finalize(&ManagedOutcome::from_status(
            r2e_core::http::StatusCode::CREATED,
        ))
        .await
        .unwrap();

        let pool = app.state().bean::<SqlitePool>().unwrap();
        let row = sqlx::query("SELECT COUNT(*) AS count FROM items")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.get::<i64, _>("count"), 1);
    }

    #[tokio::test]
    async fn rolls_back_failure_outcome() {
        let app = AppBuilder::new()
            .provide(pool_with_table().await)
            .build_state()
            .await;
        let mut tx = ManagedGuard::<SqlxTx<'static, Sqlite>, _>::acquire(ManagedContext::new(
            app.state(),
            "Test",
            "rollback",
        ))
        .await
        .unwrap();
        sqlx::query("INSERT INTO items(name) VALUES ('rolled back')")
            .execute(tx.resource_mut().connection())
            .await
            .unwrap();
        tx.finalize(&ManagedOutcome::from_status(
            r2e_core::http::StatusCode::BAD_REQUEST,
        ))
        .await
        .unwrap();

        let pool = app.state().bean::<SqlitePool>().unwrap();
        let row = sqlx::query("SELECT COUNT(*) AS count FROM items")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.get::<i64, _>("count"), 0);
    }
}
