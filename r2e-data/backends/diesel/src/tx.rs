use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use diesel::connection::TransactionManager;
use diesel::r2d2::{ConnectionManager, Pool, PooledConnection, R2D2Connection};
use diesel::Connection;
use r2e_core::{
    BeanLookup, HttpError, ManagedContext, ManagedErr, ManagedOutcome, ManagedResource,
};

use crate::DbPool;

/// Where a managed transaction gets its pool from.
///
/// Selecting the source is the only thing that differs between a transaction on
/// a fixed [`Pool`] bean ([`FixedPool`]) and one on a rotating [`DbPool`] facade
/// ([`RotatingPool`]); the connection/commit/rollback lifecycle is shared by
/// [`ManagedTx`]. Implementors look up their source bean and hand back the pool
/// to check a connection out of plus any per-transaction metadata to record.
///
/// Unlike the SQLx backend, the pool and its metadata can be handed back as a
/// plain pair: rotation here only drops the facade's handle to the old r2d2
/// pool, which stays alive and usable for as long as anyone holds a clone of
/// it, so there is no closed-pool window to retry around. The pair must still
/// come from a single [`DbPool::snapshot`] read to stay coherent.
pub trait TxSource<Conn>
where
    Conn: Connection + R2D2Connection + 'static,
{
    /// Per-transaction metadata captured at begin time: the rotating-pool
    /// generation for [`RotatingPool`], `()` for a fixed pool.
    type Meta: Copy + Send + Sync + 'static;

    /// Resolve the source bean from the request state and return the pool to
    /// check a connection out of plus the metadata to store on the transaction.
    fn acquire_pool<S>(
        context: &ManagedContext<'_, S>,
    ) -> Result<(Pool<ConnectionManager<Conn>>, Self::Meta), ManagedErr<HttpError>>
    where
        S: BeanLookup + Send + Sync;
}

/// Request-scoped Diesel transaction managed by R2E, generic over its pool
/// [`TxSource`].
///
/// Successful HTTP responses (status below 400) commit; error responses roll
/// back explicitly. An r2d2 connection with an open transaction is discarded on
/// abort rather than returned to the pool. Use it through the [`DieselTx`]/[`Tx`]
/// (fixed pool) or [`DbTx`] (rotating pool) aliases rather than spelling the
/// `Src` parameter.
pub struct ManagedTx<Conn, Src>
where
    Conn: Connection + R2D2Connection + 'static,
    Src: TxSource<Conn>,
{
    connection: Option<PooledConnection<ConnectionManager<Conn>>>,
    meta: Src::Meta,
}

/// Managed transaction on a fixed [`Pool`] bean (aliased as [`DieselTx`]).
pub type DieselTx<Conn> = ManagedTx<Conn, FixedPool<Conn>>;

/// Short name for applications depending directly on this backend crate.
pub type Tx<Conn> = DieselTx<Conn>;

/// Managed transaction on a rotating [`DbPool`] facade (aliased as [`DbTx`]).
pub type DbTx<Conn> = ManagedTx<Conn, RotatingPool<Conn>>;

impl<Conn, Src> ManagedTx<Conn, Src>
where
    Conn: Connection + R2D2Connection + Send + 'static,
    Src: TxSource<Conn>,
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

impl<Conn> ManagedTx<Conn, RotatingPool<Conn>>
where
    Conn: Connection + R2D2Connection + Send + 'static,
{
    /// The [`DbPool`] generation this transaction was begun on.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.meta
    }
}

impl<Conn, Src> Deref for ManagedTx<Conn, Src>
where
    Conn: Connection + R2D2Connection + 'static,
    Src: TxSource<Conn>,
{
    type Target = Conn;

    fn deref(&self) -> &Self::Target {
        &*self
            .connection
            .as_ref()
            .expect("managed Diesel transaction has already been finalized")
    }
}

impl<Conn, Src> DerefMut for ManagedTx<Conn, Src>
where
    Conn: Connection + R2D2Connection + 'static,
    Src: TxSource<Conn>,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut *self
            .connection
            .as_mut()
            .expect("managed Diesel transaction has already been finalized")
    }
}

impl<S, Conn, Src> ManagedResource<S> for ManagedTx<Conn, Src>
where
    S: BeanLookup + Send + Sync,
    Conn: Connection + R2D2Connection + Send + 'static,
    Src: TxSource<Conn> + 'static,
{
    type Error = ManagedErr<HttpError>;

    async fn acquire(context: ManagedContext<'_, S>) -> Result<Self, Self::Error> {
        let (pool, meta) = Src::acquire_pool(&context)?;
        let connection = run_blocking(move || {
            let mut connection = pool.get().map_err(|error| error.to_string())?;
            <Conn::TransactionManager as TransactionManager<Conn>>::begin_transaction(
                &mut connection,
            )
            .map_err(|error| error.to_string())?;
            Ok(connection)
        })
        .await?;
        Ok(Self {
            connection: Some(connection),
            meta,
        })
    }

    async fn finalize(&mut self, outcome: &ManagedOutcome) -> Result<(), Self::Error> {
        let Some(mut connection) = self.connection.take() else {
            return Ok(());
        };
        let success = outcome.is_success();
        run_blocking(move || {
            if success {
                <Conn::TransactionManager as TransactionManager<Conn>>::commit_transaction(
                    &mut connection,
                )
            } else {
                <Conn::TransactionManager as TransactionManager<Conn>>::rollback_transaction(
                    &mut connection,
                )
            }
            .map_err(|error| error.to_string())
        })
        .await
    }

    fn abort(&mut self) {
        // An r2d2 Diesel connection with an open transaction is considered
        // broken and discarded instead of being returned to the pool.
        drop(self.connection.take());
    }
}

/// [`TxSource`] backed by a fixed [`Pool`] bean.
pub struct FixedPool<Conn>(PhantomData<fn() -> Conn>);

impl<Conn> TxSource<Conn> for FixedPool<Conn>
where
    Conn: Connection + R2D2Connection + 'static,
{
    type Meta = ();

    fn acquire_pool<S>(
        context: &ManagedContext<'_, S>,
    ) -> Result<(Pool<ConnectionManager<Conn>>, ()), ManagedErr<HttpError>>
    where
        S: BeanLookup + Send + Sync,
    {
        let pool = context
            .state
            .bean::<Pool<ConnectionManager<Conn>>>()
            .ok_or_else(|| {
                context.missing_bean(
                    "database pool bean",
                    std::any::type_name::<Pool<ConnectionManager<Conn>>>(),
                    "call .provide(pool)",
                )
            })?;
        Ok((pool, ()))
    }
}

/// [`TxSource`] backed by a rotating [`DbPool`] facade.
pub struct RotatingPool<Conn>(PhantomData<fn() -> Conn>);

impl<Conn> TxSource<Conn> for RotatingPool<Conn>
where
    Conn: Connection + R2D2Connection + Send + 'static,
{
    type Meta = u64;

    fn acquire_pool<S>(
        context: &ManagedContext<'_, S>,
    ) -> Result<(Pool<ConnectionManager<Conn>>, u64), ManagedErr<HttpError>>
    where
        S: BeanLookup + Send + Sync,
    {
        let rotating = context.state.bean::<DbPool<Conn>>().ok_or_else(|| {
            context.missing_bean(
                "rotating database pool bean",
                std::any::type_name::<DbPool<Conn>>(),
                "call .register::<CreatePool>()",
            )
        })?;
        // One atomic read for both halves: reading the generation and the pool
        // separately can straddle a rotation and label the transaction with a
        // generation it never ran on.
        Ok(rotating.snapshot())
    }
}

/// Run one blocking Diesel step on Tokio's blocking pool, folding both the join
/// failure and the operation's own error string into a `ManagedErr`.
async fn run_blocking<F, T>(operation: F) -> Result<T, ManagedErr<HttpError>>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| ManagedErr(HttpError::internal(format!("Diesel task failed: {error}"))))?
        .map_err(|error| ManagedErr(HttpError::internal(error)))
}
