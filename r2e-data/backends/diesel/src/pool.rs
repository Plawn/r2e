use std::fmt;
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwap;
use diesel::r2d2::{ConnectionManager, Pool, R2D2Connection};
use diesel::Connection;
use r2e_core::config::LiveConfig;
use r2e_core::rt::CancelToken;
use r2e_core::{BeanContext, ServiceComponent};
use tokio_util::sync::CancellationToken;

/// Error building or rotating a Diesel [`DbPool`].
#[derive(Debug, Clone)]
pub struct PoolError(pub String);

impl fmt::Display for PoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PoolError {}

/// Builds a fresh r2d2 pool for a given database URL. Cloned into a blocking
/// task on every rotation, since r2d2's pool builder itself is blocking.
type PoolFactory<Conn> =
    Arc<dyn Fn(String) -> Result<Pool<ConnectionManager<Conn>>, PoolError> + Send + Sync>;

/// Diesel r2d2 pool facade that can rotate to a new underlying pool at runtime.
///
/// The active pool is swapped atomically, so `current()` stays lock-free on the
/// per-request path. Unlike SQLx's async pool, r2d2 builds pools on a blocking
/// thread, so rotation rebuilds via a [`PoolFactory`] inside `spawn_blocking`.
pub struct DbPool<Conn>
where
    Conn: Connection + R2D2Connection + 'static,
{
    inner: Arc<Inner<Conn>>,
}

impl<Conn> Clone for DbPool<Conn>
where
    Conn: Connection + R2D2Connection + 'static,
{
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
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
struct PoolGeneration<Conn>
where
    Conn: Connection + R2D2Connection + 'static,
{
    pool: Pool<ConnectionManager<Conn>>,
    generation: u64,
    url: String,
}

struct Inner<Conn>
where
    Conn: Connection + R2D2Connection + 'static,
{
    current: ArcSwap<PoolGeneration<Conn>>,
    factory: PoolFactory<Conn>,
    url: LiveConfig<String>,
    last_error: Mutex<Option<String>>,
}

impl<Conn> DbPool<Conn>
where
    Conn: Connection + R2D2Connection + Send + 'static,
{
    /// Build the initial pool from a runtime live-config URL.
    pub async fn connect(url: LiveConfig<String>) -> Result<Self, PoolError> {
        Self::connect_with(default_factory::<Conn>(), url).await
    }

    /// Build the initial pool with an explicit pool factory, e.g. to customise
    /// r2d2 pool sizing per rotation.
    pub async fn connect_with(
        factory: PoolFactory<Conn>,
        url: LiveConfig<String>,
    ) -> Result<Self, PoolError> {
        let initial_url = url.get().map_err(|error| PoolError(error.to_string()))?;
        let current = build_pool(&factory, initial_url.clone()).await?;
        Ok(Self {
            inner: Arc::new(Inner {
                current: ArcSwap::from_pointee(PoolGeneration {
                    pool: current,
                    generation: 0,
                    url: initial_url,
                }),
                factory,
                url,
                last_error: Mutex::new(None),
            }),
        })
    }

    /// Clone the currently active r2d2 pool and its generation as one
    /// consistent pair.
    #[must_use]
    pub fn snapshot(&self) -> (Pool<ConnectionManager<Conn>>, u64) {
        // Lock-free atomic load; the `Pool` is itself an `Arc` handle, so this
        // is a cheap refcount bump on the per-request path.
        let current = self.inner.current.load();
        (current.pool.clone(), current.generation)
    }

    /// Clone the currently active r2d2 pool.
    ///
    /// Prefer [`Self::snapshot`] when the generation matters: reading the pool
    /// and the generation separately can straddle a rotation.
    #[must_use]
    pub fn current(&self) -> Pool<ConnectionManager<Conn>> {
        self.inner.current.load().pool.clone()
    }

    /// Current pool generation. Incremented after each successful rotation.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.inner.current.load().generation
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
    pub async fn rotate_to(&self, url: impl Into<String>) -> Result<(), PoolError> {
        self.rotate_to_url(url.into()).await
    }

    async fn rotate_to_url(&self, url: String) -> Result<(), PoolError> {
        if self.inner.current.load().url == url {
            return Ok(());
        }
        let new_pool = build_pool(&self.inner.factory, url.clone()).await?;

        // Replacing the snapshot drops our handle to the old pool; any handle
        // handed out by `current()`/`snapshot()` and any in-flight
        // `PooledConnection` keeps the underlying r2d2 pool alive on its own,
        // so there is nothing to close and no closed-pool window to retry
        // around (unlike SQLx, whose rotation closes the old pool).
        //
        // `rcu` derives the next generation from the snapshot it replaces, so
        // the counter can never drift from the pool it labels even if two
        // rotations ever raced.
        self.inner.current.rcu(|current| {
            Arc::new(PoolGeneration {
                pool: new_pool.clone(),
                generation: current.generation + 1,
                url: url.clone(),
            })
        });
        *self.inner.last_error.lock().unwrap() = None;
        Ok(())
    }

    fn record_error(&self, error: impl ToString) {
        *self.inner.last_error.lock().unwrap() = Some(error.to_string());
    }
}

impl<Conn> fmt::Debug for DbPool<Conn>
where
    Conn: Connection + R2D2Connection + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DbPool")
            .field("generation", &self.generation())
            .field("last_error", &self.last_error())
            .finish_non_exhaustive()
    }
}

impl<Conn> ServiceComponent for DbPool<Conn>
where
    Conn: Connection + R2D2Connection + Send + 'static,
{
    /// The pool is itself the bean: `from_context` reads it back by type.
    type Deps = r2e_core::type_list::TCons<Self, r2e_core::type_list::TNil>;

    fn from_context(ctx: &BeanContext) -> Self {
        ctx.get::<Self>()
    }

    // `ServiceComponent::start` still hands over the raw tokio-util token — the
    // trait flips to `CancelToken` when r2e-core itself moves onto the facade.
    // Convert once here so the body only ever sees the facade type.
    async fn start(self, shutdown: CancellationToken) {
        let shutdown = CancelToken::from(shutdown);
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

fn default_factory<Conn>() -> PoolFactory<Conn>
where
    Conn: Connection + R2D2Connection + Send + 'static,
{
    Arc::new(|url| {
        let manager = ConnectionManager::<Conn>::new(url);
        Pool::builder()
            .build(manager)
            .map_err(|error| PoolError(error.to_string()))
    })
}

/// Run a (blocking) r2d2 pool build on Tokio's blocking pool.
async fn build_pool<Conn>(
    factory: &PoolFactory<Conn>,
    url: String,
) -> Result<Pool<ConnectionManager<Conn>>, PoolError>
where
    Conn: Connection + R2D2Connection + 'static,
{
    let factory = Arc::clone(factory);
    r2e_core::rt::spawn_blocking(move || factory(url))
        .await
        .map_err(|error| PoolError(format!("pool build task failed: {error}")))?
}
