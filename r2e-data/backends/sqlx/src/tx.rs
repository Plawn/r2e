use r2e_core::{
    BeanLookup, HttpError, ManagedContext, ManagedDeps, ManagedErr, ManagedOutcome,
    ManagedResource, TCons, TNil,
};
use sqlx::{Database, Pool, Transaction};
use std::future::Future;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use crate::{DataSourceTag, DbPool, DefaultDataSource};

/// Where a managed transaction comes from.
///
/// Selecting the source is the only thing that differs between a transaction on
/// a fixed [`Pool<DB>`] bean ([`FixedPool`]) and one on a rotating [`DbPool<DB>`]
/// facade ([`RotatingPool`]); the commit/rollback lifecycle is shared by
/// [`ManagedTx`]. Implementors look up their source bean and begin the
/// transaction themselves, returning it plus any per-transaction metadata to
/// record.
///
/// The source owns the `begin` (rather than handing a bare pool back) because
/// [`RotatingPool`] must keep the pool, the generation, and the `begin` attempt
/// together: the pool it begins on can be closed under it by a rotation, and
/// [`DbPool::begin`] retries on the replacement while reporting the generation
/// the transaction actually ran on.
pub trait TxSource<DB: Database> {
    /// Per-transaction metadata captured at begin time: the rotating-pool
    /// generation for [`RotatingPool`], the resolved tenant for
    /// [`TenantPool`](crate::TenantPool), `()` for a fixed pool.
    ///
    /// `Clone`, not `Copy`: a per-tenant transaction records its
    /// [`TenantId`](https://docs.rs/r2e-tenant) — an `Arc<str>` newtype — and
    /// nothing in the lifecycle needs the metadata to be trivially copyable.
    type Meta: Clone + Send + Sync + 'static;

    /// Type-level list ([`TCons`]/[`TNil`]) of the beans `begin` looks up.
    ///
    /// Surfaced on [`ManagedTx`] through [`ManagedDeps`], so a `#[managed]`
    /// transaction whose pool bean was never provided is a compile error at
    /// `register_controller` instead of a 500 on the first request.
    type Deps;

    /// Resolve the source bean from the request state and begin a transaction
    /// on it, returning it with the metadata to store on it.
    fn begin<S>(
        context: &ManagedContext<'_, S>,
    ) -> impl Future<Output = Result<(Transaction<'static, DB>, Self::Meta), ManagedErr<HttpError>>> + Send
    where
        S: BeanLookup + Send + Sync;
}

/// Request-scoped SQLx transaction managed by R2E, generic over its pool
/// [`TxSource`].
///
/// Successful HTTP responses (status below 400) commit; error responses roll
/// back explicitly. Dropping an unfinished transaction provides SQLx's rollback
/// fallback. Use it through the [`SqlxTx`]/[`Tx`] (fixed pool) or [`DbTx`]
/// (rotating pool) aliases rather than spelling the `Src` parameter.
pub struct ManagedTx<'a, DB: Database, Src: TxSource<DB>> {
    inner: Option<Transaction<'a, DB>>,
    meta: Src::Meta,
}

/// Managed transaction on a fixed [`Pool<DB>`] bean (aliased as [`SqlxTx`]).
pub type SqlxTx<'a, DB> = ManagedTx<'a, DB, FixedPool<DB>>;

/// Backward-compatible short name used in handler signatures.
pub type Tx<'a, DB> = SqlxTx<'a, DB>;

/// Managed transaction on a rotating [`DbPool<DB>`] facade (aliased as [`DbTx`]).
///
/// `Tag` names the datasource, and defaults to the app's unnamed one — a
/// transaction on a named datasource is `DbTx<'_, DB, Reporting>`.
pub type DbTx<'a, DB, Tag = DefaultDataSource> = ManagedTx<'a, DB, RotatingPool<DB, Tag>>;

impl<'a, DB: Database, Src: TxSource<DB>> ManagedTx<'a, DB, Src> {
    pub fn connection(&mut self) -> &mut DB::Connection {
        self.as_mut()
    }

    pub fn as_mut(&mut self) -> &mut DB::Connection {
        &mut *self
            .inner
            .as_mut()
            .expect("managed SQLx transaction has already been finalized")
    }

    /// The metadata the [`TxSource`] recorded when it began this transaction.
    ///
    /// Sources expose it under a domain name — [`generation`](ManagedTx::generation)
    /// on a rotating-pool transaction, `tenant()` on a per-tenant one — and this
    /// is the generic accessor those are written on top of.
    #[must_use]
    pub fn meta(&self) -> &Src::Meta {
        &self.meta
    }
}

impl<'a, DB: Database, Tag: DataSourceTag> ManagedTx<'a, DB, RotatingPool<DB, Tag>> {
    /// The [`DbPool`] generation this transaction was begun on.
    #[must_use]
    pub fn generation(&self) -> u64 {
        *self.meta()
    }
}

impl<'a, DB: Database, Src: TxSource<DB>> Deref for ManagedTx<'a, DB, Src> {
    type Target = Transaction<'a, DB>;

    fn deref(&self) -> &Self::Target {
        self.inner
            .as_ref()
            .expect("managed SQLx transaction has already been finalized")
    }
}

impl<'a, DB: Database, Src: TxSource<DB>> DerefMut for ManagedTx<'a, DB, Src> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner
            .as_mut()
            .expect("managed SQLx transaction has already been finalized")
    }
}

impl<'a, DB: Database, Src: TxSource<DB>> ManagedDeps for ManagedTx<'a, DB, Src> {
    type Deps = Src::Deps;
}

impl<S, DB, Src> ManagedResource<S> for ManagedTx<'static, DB, Src>
where
    DB: Database,
    S: BeanLookup + Send + Sync,
    Src: TxSource<DB> + 'static,
{
    type Error = ManagedErr<HttpError>;

    async fn acquire(context: ManagedContext<'_, S>) -> Result<Self, Self::Error> {
        let (transaction, meta) = Src::begin(&context).await?;
        Ok(Self {
            inner: Some(transaction),
            meta,
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

/// [`TxSource`] backed by a fixed [`Pool<DB>`] bean.
pub struct FixedPool<DB>(PhantomData<fn() -> DB>);

impl<DB: Database> TxSource<DB> for FixedPool<DB> {
    type Meta = ();
    type Deps = TCons<Pool<DB>, TNil>;

    async fn begin<S>(
        context: &ManagedContext<'_, S>,
    ) -> Result<(Transaction<'static, DB>, ()), ManagedErr<HttpError>>
    where
        S: BeanLookup + Send + Sync,
    {
        let pool = context.state.bean::<Pool<DB>>().ok_or_else(|| {
            context.missing_bean(
                "database pool bean",
                std::any::type_name::<Pool<DB>>(),
                "call .provide(pool)",
            )
        })?;
        let transaction = pool.begin().await.map_err(begin_failed)?;
        Ok((transaction, ()))
    }
}

/// [`TxSource`] backed by a rotating [`DbPool<DB, Tag>`] facade.
pub struct RotatingPool<DB, Tag = DefaultDataSource>(PhantomData<fn() -> (DB, Tag)>);

impl<DB: Database, Tag: DataSourceTag> TxSource<DB> for RotatingPool<DB, Tag> {
    type Meta = u64;
    type Deps = TCons<DbPool<DB, Tag>, TNil>;

    async fn begin<S>(
        context: &ManagedContext<'_, S>,
    ) -> Result<(Transaction<'static, DB>, u64), ManagedErr<HttpError>>
    where
        S: BeanLookup + Send + Sync,
    {
        let rotating = context.state.bean::<DbPool<DB, Tag>>().ok_or_else(|| {
            context.missing_bean(
                "rotating database pool bean",
                std::any::type_name::<DbPool<DB, Tag>>(),
                "install the datasource plugin: .plugin(SqlxDataSource::<DB>::new())",
            )
        })?;
        // `DbPool::begin` takes the (pool, generation) pair atomically and
        // retries if a rotation closed the pool between the snapshot and the
        // begin, so the reported generation always matches the transaction.
        rotating.begin().await.map_err(begin_failed)
    }
}

/// Map an SQLx begin failure onto the managed-resource error type.
fn begin_failed(error: sqlx::Error) -> ManagedErr<HttpError> {
    ManagedErr(HttpError::internal(error.to_string()))
}
