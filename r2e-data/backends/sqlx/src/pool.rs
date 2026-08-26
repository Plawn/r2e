use std::fmt;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwap;
use async_stream::try_stream;
use futures_core::future::BoxFuture;
use futures_core::stream::BoxStream;
use futures_util::TryStreamExt;
use r2e_core::config::LiveConfig;
use r2e_core::rt::CancelToken;
use r2e_core::{BeanContext, ConfigError, ServiceComponent};

use crate::datasource::{DataSourceTag, DefaultDataSource};
use sqlx::pool::{PoolConnection, PoolOptions};
use sqlx::{Database, Either, Error, Execute, Executor, Pool, SqlStr, Transaction};

/// How many times a `begin`/`acquire` is retried when it lands on a pool that
/// was closed by a concurrent rotation.
///
/// `rotate_to_url` publishes the replacement snapshot *before* closing the old
/// pool, so one retry is normally enough; the extra attempts only cover
/// back-to-back rotations. Bounding this keeps a permanently-closed pool from
/// spinning forever.
const MAX_POOL_ATTEMPTS: usize = 3;

/// SQLx pool facade that can rotate to a new underlying pool at runtime.
///
/// `Tag` names *which* datasource this pool is: [`DefaultDataSource`] (the
/// default) is the app's single, unnamed database — `DbPool<Postgres>` means
/// exactly what it always did. A named tag (see
/// [`datasource_tag!`](crate::datasource_tag)) makes a second, distinct bean
/// type so several datasources can coexist in one app, each with its own
/// `datasource.<name>.*` config section. The tag is a compile-time marker only:
/// it costs nothing at runtime and never reaches SQLx.
pub struct DbPool<DB: Database, Tag = DefaultDataSource> {
    inner: Arc<Inner<DB>>,
    /// `fn() -> Tag` (not `Tag`): the marker must not make the pool's
    /// auto-traits depend on it, and the pool never holds a `Tag` value.
    tag: PhantomData<fn() -> Tag>,
}

impl<DB: Database, Tag> Clone for DbPool<DB, Tag> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            tag: PhantomData,
        }
    }
}

/// The active pool together with the generation it was published under.
///
/// Pool and generation live in a single `ArcSwap` cell so that readers can
/// never observe a torn pair: taking the generation and the pool as two
/// separate atomic reads let a rotation slip in between and label a
/// transaction with a generation it did not run on.
///
/// The URL rides along for the same reason: it is what the generation was built
/// from, so keeping it in the cell makes "are we already on this URL?" a
/// lock-free read of the very snapshot it describes.
struct PoolGeneration<DB: Database> {
    pool: Pool<DB>,
    generation: u64,
    url: String,
}

struct Inner<DB: Database> {
    current: ArcSwap<PoolGeneration<DB>>,
    options: PoolOptions<DB>,
    url: LiveConfig<String>,
    last_error: Mutex<Option<String>>,
}

impl<DB: Database, Tag> DbPool<DB, Tag> {
    /// Build the initial pool from a runtime live-config URL.
    pub async fn connect(url: LiveConfig<String>) -> Result<Self, Error> {
        Self::connect_with(PoolOptions::<DB>::new(), url).await
    }

    /// Build the initial pool with explicit SQLx pool options.
    pub async fn connect_with(
        options: PoolOptions<DB>,
        url: LiveConfig<String>,
    ) -> Result<Self, Error> {
        let initial_url = url.get().map_err(config_to_sqlx)?;
        let current = options.clone().connect(&initial_url).await?;
        Ok(Self {
            inner: Arc::new(Inner {
                current: ArcSwap::from_pointee(PoolGeneration {
                    pool: current,
                    generation: 0,
                    url: initial_url,
                }),
                options,
                url,
                last_error: Mutex::new(None),
            }),
            tag: PhantomData,
        })
    }

    /// Clone the currently active SQLx pool and its generation as one
    /// consistent pair.
    #[must_use]
    pub fn snapshot(&self) -> (Pool<DB>, u64) {
        // Lock-free atomic load; the `Pool` is itself an `Arc` handle, so this
        // is a cheap refcount bump on the per-query/per-request path.
        let current = self.inner.current.load();
        (current.pool.clone(), current.generation)
    }

    /// Clone the currently active SQLx pool.
    ///
    /// Prefer [`Self::snapshot`] (or [`Self::begin`]) when the generation
    /// matters: reading the pool and the generation separately can straddle a
    /// rotation.
    #[must_use]
    pub fn current(&self) -> Pool<DB> {
        self.inner.current.load().pool.clone()
    }

    /// Current pool generation. Incremented after each successful rotation.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.inner.current.load().generation
    }

    /// Begin a transaction on the active pool, returning it with the generation
    /// it actually ran on.
    ///
    /// A rotation closes the previous pool right after publishing the new one,
    /// so a snapshot taken microseconds earlier can fail with
    /// [`Error::PoolClosed`]. That is a rotation artefact, not a database
    /// failure: re-read the snapshot and retry, bounded to three attempts so a
    /// genuinely closed pool still surfaces its error.
    pub async fn begin(&self) -> Result<(Transaction<'static, DB>, u64), Error> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let (pool, generation) = self.snapshot();
            match pool.begin().await {
                Ok(transaction) => return Ok((transaction, generation)),
                Err(Error::PoolClosed) if attempt < MAX_POOL_ATTEMPTS => {}
                Err(error) => return Err(error),
            }
        }
    }

    /// Check out a connection from the active pool, retrying across a rotation
    /// exactly like [`Self::begin`].
    async fn acquire_with_retry(&self) -> Result<PoolConnection<DB>, Error> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let pool = self.current();
            match pool.acquire().await {
                Ok(connection) => return Ok(connection),
                Err(Error::PoolClosed) if attempt < MAX_POOL_ATTEMPTS => {}
                Err(error) => return Err(error),
            }
        }
    }

    /// Last rotation error, if any.
    #[must_use]
    pub fn last_error(&self) -> Option<String> {
        self.inner.last_error.lock().unwrap().clone()
    }

    /// Rotate to `url` now, bypassing the live-config watch loop.
    ///
    /// Exposed for tests and operational tooling; the supported path is the
    /// [`ServiceComponent`] loop reacting to the live-config value.
    #[doc(hidden)]
    pub async fn rotate_to(&self, url: impl Into<String>) -> Result<(), Error> {
        self.rotate_to_url(url.into()).await
    }

    async fn rotate_to_url(&self, url: String) -> Result<(), Error> {
        if self.inner.current.load().url == url {
            return Ok(());
        }
        let new_pool = self.inner.options.clone().connect(&url).await?;

        // `rcu` derives the next generation from the snapshot it replaces, so
        // the counter can never drift from the pool it labels even if two
        // rotations ever raced.
        let old = self.inner.current.rcu(|current| {
            Arc::new(PoolGeneration {
                pool: new_pool.clone(),
                generation: current.generation + 1,
                url: url.clone(),
            })
        });
        *self.inner.last_error.lock().unwrap() = None;

        r2e_core::rt::spawn(async move {
            old.pool.close().await;
        });
        Ok(())
    }

    fn record_error(&self, error: impl ToString) {
        *self.inner.last_error.lock().unwrap() = Some(error.to_string());
    }
}

impl<DB: Database, Tag> fmt::Debug for DbPool<DB, Tag> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DbPool")
            .field("generation", &self.generation())
            .field("last_error", &self.last_error())
            .finish_non_exhaustive()
    }
}

impl<DB, Tag> ServiceComponent for DbPool<DB, Tag>
where
    DB: Database,
    Tag: DataSourceTag,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
{
    /// The pool is itself the bean: `from_context` reads it back by type.
    type Deps = r2e_core::type_list::TCons<Self, r2e_core::type_list::TNil>;

    fn from_context(ctx: &BeanContext) -> Self {
        ctx.get::<Self>()
    }

    async fn start(self, shutdown: CancelToken) {
        let this = &self;
        this.inner
            .url
            .subscribe()
            .drive(shutdown, move |url| async move {
                match url {
                    Ok(url) => {
                        if let Err(error) = this.rotate_to_url(url).await {
                            this.record_error(error);
                        }
                    }
                    Err(error) => this.record_error(error),
                }
            })
            .await;
    }
}

impl<'p, DB, Tag> Executor<'p> for &'_ DbPool<DB, Tag>
where
    DB: Database,
    Tag: DataSourceTag,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
{
    type Database = DB;

    fn fetch_many<'e, 'q: 'e, E>(
        self,
        query: E,
    ) -> BoxStream<'e, Result<Either<DB::QueryResult, DB::Row>, Error>>
    where
        E: 'q + Execute<'q, Self::Database>,
    {
        // The facade (not a pre-resolved pool) is moved into the future so the
        // snapshot is taken — and retried across a rotation — at poll time.
        let facade = (*self).clone();

        Box::pin(try_stream! {
            let mut conn = facade.acquire_with_retry().await?;
            let mut stream = conn.fetch_many(query);

            while let Some(value) = stream.try_next().await? {
                yield value;
            }
        })
    }

    fn fetch_optional<'e, 'q: 'e, E>(
        self,
        query: E,
    ) -> BoxFuture<'e, Result<Option<DB::Row>, Error>>
    where
        E: 'q + Execute<'q, Self::Database>,
    {
        let facade = (*self).clone();
        Box::pin(async move {
            facade
                .acquire_with_retry()
                .await?
                .fetch_optional(query)
                .await
        })
    }

    fn prepare_with<'e>(
        self,
        sql: SqlStr,
        parameters: &'e [<Self::Database as Database>::TypeInfo],
    ) -> BoxFuture<'e, Result<<Self::Database as Database>::Statement, Error>>
    where
        'p: 'e,
    {
        let facade = (*self).clone();
        Box::pin(async move {
            facade
                .acquire_with_retry()
                .await?
                .prepare_with(sql, parameters)
                .await
        })
    }

    fn describe<'e>(
        self,
        sql: SqlStr,
    ) -> BoxFuture<'e, Result<sqlx::Describe<Self::Database>, Error>> {
        let facade = (*self).clone();
        Box::pin(async move { facade.acquire_with_retry().await?.describe(sql).await })
    }
}

fn config_to_sqlx(error: ConfigError) -> Error {
    Error::Configuration(Box::new(error))
}
