//! Readiness health check for a datasource: `SELECT 1` on the pool.
//!
//! ```ignore
//! AppBuilder::new()
//!     .load_config::<()>()
//!     .plugin(SqlxDataSource::<sqlx::Postgres>::new())
//!     .plugin(Health::builder().build())          // provides `HealthRegistry`
//!     .plugin(DataSourceHealth::<sqlx::Postgres>::new())
//!     .build_state()
//!     .await;
//! ```
//!
//! The check is named `db` for the default datasource and `db:<name>` for a
//! named [`DataSourceTag`], and it counts towards `/health/ready` (call
//! [`DataSourceHealth::liveness_only`] to keep it out of readiness).

use std::marker::PhantomData;

use r2e_core::builtins::health::{HealthIndicator, HealthRegistry, HealthStatus};
use r2e_core::plugin::{Plugin, PluginBuildContext, PluginBuildError};
use sqlx::{Database, Executor};

use crate::{DataSourceTag, DbPool, DefaultDataSource};

/// Plugin that registers a `SELECT 1` health check for `DbPool<DB, Tag>`.
///
/// It provides nothing and only *contributes* to the health registry, so it
/// needs both the pool and the registry: install it after
/// [`SqlxDataSource`](crate::SqlxDataSource) and an
/// [`AdvancedHealth`](r2e_core::AdvancedHealth) (`Health::builder()…`) plugin —
/// a missing `HealthRegistry` bean is a compile error.
pub struct DataSourceHealth<DB: Database, Tag = DefaultDataSource> {
    name: Option<String>,
    affects_readiness: bool,
    marker: PhantomData<fn() -> (DB, Tag)>,
}

impl<DB: Database, Tag> Default for DataSourceHealth<DB, Tag> {
    fn default() -> Self {
        Self::new()
    }
}

impl<DB: Database, Tag> DataSourceHealth<DB, Tag> {
    /// A readiness check named after the datasource.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            name: None,
            affects_readiness: true,
            marker: PhantomData,
        }
    }

    /// Override the check's name (default: `db`, or `db:<tag>`).
    #[must_use]
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Report the check on `/health` but keep it out of `/health/ready`.
    #[must_use]
    pub const fn liveness_only(mut self) -> Self {
        self.affects_readiness = false;
        self
    }
}

impl<DB, Tag> Plugin for DataSourceHealth<DB, Tag>
where
    DB: Database,
    Tag: DataSourceTag,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
{
    type Provided = ();
    type Deps = (DbPool<DB, Tag>, HealthRegistry);
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        (pool, registry): Self::Deps,
        _config: Option<Self::Config>,
        ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        if !ctx.enabled() {
            return Ok(());
        }
        let name = self.name.unwrap_or_else(|| match Tag::NAME {
            Some(tag) => format!("db:{tag}"),
            None => "db".to_string(),
        });
        registry.register(PoolHealth {
            name,
            pool,
            affects_readiness: self.affects_readiness,
        });
        Ok(())
    }
}

struct PoolHealth<DB: Database, Tag> {
    name: String,
    pool: DbPool<DB, Tag>,
    affects_readiness: bool,
}

impl<DB, Tag> HealthIndicator for PoolHealth<DB, Tag>
where
    DB: Database,
    Tag: Send + Sync + 'static,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
{
    fn name(&self) -> &str {
        &self.name
    }

    fn check(&self) -> impl std::future::Future<Output = HealthStatus> + Send {
        let pool = self.pool.current();
        async move {
            // Raw SQL through `Executor` (not `sqlx::query`): it needs no
            // `IntoArguments` bound, so the check stays generic over `DB`.
            match Executor::execute(&pool, "SELECT 1").await {
                Ok(_) => HealthStatus::Up,
                Err(error) => HealthStatus::Down(error.to_string()),
            }
        }
    }

    fn affects_readiness(&self) -> bool {
        self.affects_readiness
    }
}
