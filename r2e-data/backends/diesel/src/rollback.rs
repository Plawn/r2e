//! Rollback-only pools: what a test writes never reaches the database.
//!
//! A rollback-only pool holds **one** connection, put in Diesel's test
//! transaction as soon as it connects. Everything the app does through the
//! pool happens inside that transaction — a managed [`Tx`](crate::Tx) /
//! [`DbTx`](crate::DbTx) becomes a savepoint in it — and the database rolls
//! the whole thing back when the pool goes away. Tests can then share one
//! migrated database without seeing each other's rows (Quarkus:
//! `@TestTransaction`).

use std::collections::HashSet;
use std::sync::Arc;

use diesel::r2d2::{ConnectionManager, CustomizeConnection, Pool, R2D2Connection};
use diesel::Connection;
use r2e_core::{LiveConfigRegistry, R2eConfig};

use crate::pool::build_pool;
use crate::{DataSourceTag, DbPool, PoolError, PoolFactory};

/// Opens the transaction nothing commits. `begin_test_transaction` rather than
/// a plain `BEGIN`: Diesel flags the transaction as a test one, so r2d2 does
/// not take a connection returned mid-transaction for a broken one.
#[derive(Debug)]
struct RollbackOnly;

impl<Conn: Connection> CustomizeConnection<Conn, diesel::r2d2::Error> for RollbackOnly {
    fn on_acquire(&self, connection: &mut Conn) -> Result<(), diesel::r2d2::Error> {
        connection
            .begin_test_transaction()
            .map_err(diesel::r2d2::Error::QueryError)
    }
}

/// The pool factory of a rollback-only pool: a single connection, never
/// recycled.
fn rollback_only_factory<Conn>() -> PoolFactory<Conn>
where
    Conn: Connection + R2D2Connection + Send + 'static,
{
    Arc::new(|url| {
        Pool::builder()
            .max_size(1)
            // Recycling the connection would silently end the transaction and
            // start an empty one.
            .idle_timeout(None)
            .max_lifetime(None)
            .connection_customizer(Box::new(RollbackOnly))
            .build(ConnectionManager::<Conn>::new(url))
            .map_err(|error| PoolError(error.to_string()))
    })
}

/// Connect a rollback-only r2d2 pool: nothing written through it is ever
/// committed.
///
/// For tests of an app whose pool bean is a plain
/// `Pool<ConnectionManager<Conn>>`; an app on the datasource plugin pins
/// [`DbPool::rollback_only`] instead. See there for what the mode implies.
pub async fn rollback_only_pool<Conn>(
    url: impl Into<String>,
) -> Result<Pool<ConnectionManager<Conn>>, PoolError>
where
    Conn: Connection + R2D2Connection + Send + 'static,
{
    build_pool(&rollback_only_factory::<Conn>(), url.into()).await
}

impl<Conn, Tag> DbPool<Conn, Tag>
where
    Conn: Connection + R2D2Connection + Send + 'static,
    Tag: DataSourceTag,
{
    /// Connect a pool on which nothing is ever committed — for tests.
    ///
    /// Pin it in place of the datasource's pool and every write the app makes
    /// during the test is rolled back when the pool is dropped, so tests can
    /// share one migrated database:
    ///
    /// ```ignore
    /// struct Db(DbPool<PgConnection>);
    ///
    /// impl TestResource for Db {
    ///     async fn start() -> Self {
    ///         let url = migrated_database().await;
    ///         Db(DbPool::rollback_only(url).await.expect("test database"))
    ///     }
    ///
    ///     fn configure(&self, builder: AppBuilder) -> AppBuilder {
    ///         builder.override_bean(self.0.clone())
    ///     }
    /// }
    /// ```
    ///
    /// The pool holds **one** connection with an open transaction; a managed
    /// transaction of the app is a savepoint inside it, committed or rolled
    /// back as usual from the app's point of view. What follows from that:
    ///
    /// - The test reads its own writes through this pool, and through this
    ///   pool only — another connection sees none of them.
    /// - Everything is serialized on the one connection. Code that holds a
    ///   connection and asks the pool for a *second* one waits for itself
    ///   until the checkout times out.
    /// - Migrate beforehand, on an ordinary connection: a pinned pool skips
    ///   the plugin's `migrate-at-start`, and a migration run through this
    ///   pool would be rolled back with the rest.
    /// - On PostgreSQL a failed statement outside any savepoint aborts the
    ///   outer transaction, and every later statement fails with it. A
    ///   statement expected to fail belongs in a transaction of the app.
    /// - Sequences and anything else the database does not roll back keep
    ///   moving.
    ///
    /// The pool never rotates: there is no live URL behind it.
    pub async fn rollback_only(url: impl Into<String>) -> Result<Self, PoolError> {
        let url = url.into();
        // The facade wants a live URL; this one never changes.
        let key = format!("{}.url", Tag::CONFIG_PREFIX);
        let mut config = R2eConfig::empty();
        config.set(&key, url.as_str().into());
        let live_url = LiveConfigRegistry::from_config(&config, HashSet::new()).live_config(&key);
        Self::connect_with(rollback_only_factory::<Conn>(), live_url).await
    }
}
