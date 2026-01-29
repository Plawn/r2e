use std::sync::Arc;

use quarlus_core::prelude::*;
use quarlus_core::{AppError, ManagedError, ManagedResource, QuarlusConfig};
use quarlus_events::EventBus;
use quarlus_rate_limit::RateLimitRegistry;
use quarlus_security::JwtClaimsValidator;
use sqlx::{Database, Pool, Sqlite, Transaction};
use std::ops::{Deref, DerefMut};
use tokio_util::sync::CancellationToken;

use crate::services::UserService;

#[derive(Clone, BeanState)]
pub struct Services {
    pub user_service: UserService,
    pub claims_validator: Arc<JwtClaimsValidator>,
    pub pool: sqlx::SqlitePool,
    pub event_bus: EventBus,
    pub config: QuarlusConfig,
    pub cancel: CancellationToken,
    pub rate_limiter: RateLimitRegistry,
}

// ─────────────────────────────────────────────────────────────────────────────
// Transaction wrapper with ManagedResource implementation
// ─────────────────────────────────────────────────────────────────────────────

/// A wrapper around SQLx `Transaction` for use with `#[managed]`.
///
/// When used with `#[managed]`, the transaction is:
/// - Acquired (begun) before the handler executes
/// - Committed if the handler returns `Ok` (or a non-Result type)
/// - Rolled back (on drop) if the handler returns `Err` or panics
pub struct Tx<'a, DB: Database>(pub Transaction<'a, DB>);

impl<'a, DB: Database> Deref for Tx<'a, DB> {
    type Target = Transaction<'a, DB>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a, DB: Database> DerefMut for Tx<'a, DB> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<'a, DB: Database> Tx<'a, DB> {
    /// Unwraps the `Tx` into the inner `Transaction`.
    pub fn into_inner(self) -> Transaction<'a, DB> {
        self.0
    }

    /// Returns a mutable reference to the underlying connection.
    pub fn as_mut(&mut self) -> &mut <DB as Database>::Connection {
        &mut *self.0
    }
}

/// Trait for application states that contain a database pool.
pub trait HasPool<DB: Database> {
    fn pool(&self) -> &Pool<DB>;
}

impl HasPool<Sqlite> for Services {
    fn pool(&self) -> &Pool<Sqlite> {
        &self.pool
    }
}

/// ManagedResource implementation for Tx - handles transaction lifecycle.
///
/// - `acquire`: begins a new transaction from the pool
/// - `release(true)`: commits the transaction
/// - `release(false)`: drops the transaction (automatic rollback)
impl<S, DB> ManagedResource<S> for Tx<'static, DB>
where
    DB: Database,
    S: HasPool<DB> + Send + Sync,
{
    type Error = ManagedError;

    async fn acquire(state: &S) -> Result<Self, Self::Error> {
        let tx = state
            .pool()
            .begin()
            .await
            .map_err(|e| ManagedError(AppError::Internal(e.to_string())))?;
        Ok(Tx(tx))
    }

    async fn release(self, success: bool) -> Result<(), Self::Error> {
        if success {
            self.into_inner()
                .commit()
                .await
                .map_err(|e| ManagedError(AppError::Internal(e.to_string())))?;
        }
        // If !success, the transaction is dropped and automatically rolled back
        Ok(())
    }
}
