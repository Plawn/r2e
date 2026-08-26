//! Readiness health check for a datasource: check out a connection (r2d2 pings
//! it on checkout, Diesel's `SELECT 1` equivalent).
//!
//! ```ignore
//! AppBuilder::new()
//!     .load_config::<()>()
//!     .plugin(DieselDataSource::<SqliteConnection>::new())
//!     .plugin(Health::builder().build())            // provides `HealthRegistry`
//!     .plugin(DataSourceHealth::<SqliteConnection>::new())
//!     .build_state()
//!     .await;
//! ```
//!
//! The check is named `db` for the default datasource and `db:<name>` for a
//! named [`DataSourceTag`], and it counts towards `/health/ready` (call
//! [`DataSourceHealth::liveness_only`] to keep it out of readiness).

use std::marker::PhantomData;

use diesel::r2d2::R2D2Connection;
use diesel::Connection;
use r2e_core::builtins::health::{HealthIndicator, HealthRegistry, HealthStatus};
use r2e_core::plugin::{Plugin, PluginBuildContext, PluginBuildError};

use crate::{DataSourceTag, DbPool, DefaultDataSource};

/// Plugin that registers a connectivity health check for `DbPool<Conn, Tag>`.
///
/// It provides nothing and only *contributes* to the health registry, so it
/// needs both the pool and the registry: install it after
/// [`DieselDataSource`](crate::DieselDataSource) and an
/// [`AdvancedHealth`](r2e_core::AdvancedHealth) (`Health::builder()…`) plugin —
/// a missing `HealthRegistry` bean is a compile error.
pub struct DataSourceHealth<Conn, Tag = DefaultDataSource> {
    name: Option<String>,
    affects_readiness: bool,
    marker: PhantomData<fn() -> (Conn, Tag)>,
}

impl<Conn, Tag> Default for DataSourceHealth<Conn, Tag> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Conn, Tag> DataSourceHealth<Conn, Tag> {
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

impl<Conn, Tag> Plugin for DataSourceHealth<Conn, Tag>
where
    Conn: Connection + R2D2Connection + Send + 'static,
    Tag: DataSourceTag,
{
    type Provided = ();
    type Deps = (DbPool<Conn, Tag>, HealthRegistry);
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

struct PoolHealth<Conn, Tag>
where
    Conn: Connection + R2D2Connection + 'static,
{
    name: String,
    pool: DbPool<Conn, Tag>,
    affects_readiness: bool,
}

impl<Conn, Tag> HealthIndicator for PoolHealth<Conn, Tag>
where
    Conn: Connection + R2D2Connection + Send + 'static,
    Tag: Send + Sync + 'static,
{
    fn name(&self) -> &str {
        &self.name
    }

    fn check(&self) -> impl std::future::Future<Output = HealthStatus> + Send {
        let pool = self.pool.current();
        async move {
            // Diesel is blocking: check out on the blocking pool. r2d2 pings
            // the connection on checkout, so this *is* the `SELECT 1`.
            match r2e_core::rt::spawn_blocking(move || pool.get().map(|_| ())).await {
                Ok(Ok(())) => HealthStatus::Up,
                Ok(Err(error)) => HealthStatus::Down(error.to_string()),
                Err(error) => HealthStatus::Down(format!("Diesel task failed: {error}")),
            }
        }
    }

    fn affects_readiness(&self) -> bool {
        self.affects_readiness
    }
}
