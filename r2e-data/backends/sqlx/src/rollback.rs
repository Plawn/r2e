//! Rollback-only pools: what a test writes never reaches the database.
//!
//! A rollback-only pool holds **one** connection, and opens a transaction on
//! it that nothing ever commits. Everything the app does through the pool
//! happens inside that transaction — a `pool.begin()` (and so every managed
//! [`Tx`](crate::Tx) / [`DbTx`](crate::DbTx)) becomes a savepoint in it — and
//! the database rolls the whole thing back when the pool goes away. Tests can
//! then share one migrated database without seeing each other's rows
//! (Quarkus: `@TestTransaction`).

use std::collections::HashSet;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwap;
use r2e_core::{LiveConfigRegistry, R2eConfig};
use sqlx::pool::PoolOptions;
use sqlx::{Connection, Database, Error, Pool};

use crate::pool::{Inner, PoolGeneration};
use crate::{DataSourceTag, DbPool};

/// Pool options for a rollback-only pool: a single connection, never recycled,
/// with a transaction opened on it as soon as it connects.
fn rollback_only_options<DB: Database>() -> PoolOptions<DB> {
    PoolOptions::<DB>::new()
        .max_connections(1)
        // Recycling the connection would silently end the transaction and
        // start an empty one.
        .idle_timeout(None)
        .max_lifetime(None)
        .after_connect(|connection, _| {
            Box::pin(async move {
                // `begin` rather than `BEGIN` as SQL: SQLx has to know the
                // connection is one level deep for the app's own `begin` to
                // issue a savepoint instead of a second `BEGIN`.
                let transaction = connection.begin().await?;
                // Dropping it would queue a rollback. Forgetting it leaves the
                // transaction open for as long as the connection lives; the
                // guard owns nothing but a borrow, so nothing leaks.
                std::mem::forget(transaction);
                Ok(())
            })
        })
}

/// Connect a rollback-only [`Pool<DB>`]: nothing written through it is ever
/// committed.
///
/// For tests of an app whose pool bean is a plain `sqlx::Pool<DB>`; an app on
/// the datasource plugin pins [`DbPool::rollback_only`] instead. See there for
/// what the mode implies.
pub async fn rollback_only_pool<DB: Database>(url: &str) -> Result<Pool<DB>, Error> {
    rollback_only_options::<DB>().connect(url).await
}

impl<DB: Database, Tag: DataSourceTag> DbPool<DB, Tag> {
    /// Connect a pool on which nothing is ever committed — for tests.
    ///
    /// Pin it in place of the datasource's pool and every write the app makes
    /// during the test is rolled back when the pool is dropped, so tests can
    /// share one migrated database:
    ///
    /// ```ignore
    /// struct Db(DbPool<Postgres>);
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
    ///   transaction and asks the pool for a *second* connection waits for
    ///   itself until the acquire times out.
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
    pub async fn rollback_only(url: impl Into<String>) -> Result<Self, Error> {
        let url = url.into();
        let options = rollback_only_options::<DB>();
        let pool = options.clone().connect(&url).await?;

        // The facade wants a live URL; this one never changes.
        let key = format!("{}.url", Tag::CONFIG_PREFIX);
        let mut config = R2eConfig::empty();
        config.set(&key, url.as_str().into());
        let live_url = LiveConfigRegistry::from_config(&config, HashSet::new()).live_config(&key);

        Ok(Self {
            inner: Arc::new(Inner {
                current: ArcSwap::from_pointee(PoolGeneration {
                    pool,
                    generation: 0,
                    url,
                }),
                options,
                url: live_url,
                last_error: Mutex::new(None),
            }),
            tag: PhantomData,
        })
    }
}
