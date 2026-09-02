//! Scaffolding for `llm/tenancy-datasources.md`.
//!
//! Mirrors `examples/example-multi-tenant-db`: a master directory bean, the
//! per-tenant branding source, and (for the Diesel half) a tiny `orders` table.

use r2e::prelude::*;
use r2e::tenant::{BoxError, BoxFuture, TenantContext, TenantId, TenantSource};
use sqlx::{Pool, Sqlite};

pub use std::time::Duration;

/// `diesel::prelude::*` is what brings `RunQueryDsl` (`.execute`) into scope.
pub use diesel::prelude::*;

// ── The master directory: the one table that says which tenants exist ───────

/// The app-scoped master database (`examples/example-multi-tenant-db`'s
/// `MasterDb`): slug → DSN, API token, optional theme.
#[derive(Clone)]
pub struct MasterDb(Pool<Sqlite>);

impl MasterDb {
    /// The tenant's own database URL — `None` for an unknown tenant.
    pub async fn dsn(&self, slug: &str) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar("SELECT dsn FROM tenants WHERE slug = ?")
            .bind(slug)
            .fetch_optional(&self.0)
            .await
    }

    /// Same lookup, named as the Diesel block spells it.
    pub async fn dsn_for(&self, slug: &str) -> Result<Option<String>, sqlx::Error> {
        self.dsn(slug).await
    }

    /// The tenant's API credentials — feeds the cascade demo.
    pub async fn api_token(&self, slug: &str) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar("SELECT api_token FROM tenants WHERE slug = ?")
            .bind(slug)
            .fetch_optional(&self.0)
            .await
    }

    /// The tenant's custom theme, if it has one — feeds the fallback demo.
    pub async fn theme(&self, slug: &str) -> Result<Option<String>, sqlx::Error> {
        let row: Option<Option<String>> =
            sqlx::query_scalar("SELECT theme FROM tenants WHERE slug = ?")
                .bind(slug)
                .fetch_optional(&self.0)
                .await?;
        Ok(row.flatten())
    }
}

/// The `App::Env` bundle: `setup()` provisions the directory once.
#[derive(Clone)]
pub struct AppEnv {
    pub master: MasterDb,
}

/// Creates/opens the master database — the app's provisioning path.
pub async fn provision() -> Result<MasterDb, BootError> {
    Ok(MasterDb(Pool::<Sqlite>::connect_lazy("sqlite::memory:")?))
}

// ── SPI: the fallback source (a tenant without a theme gets the shared bean) ─

/// The per-tenant (or app-scoped default) branding.
#[derive(Clone)]
pub struct Branding {
    pub theme: String,
}

impl Branding {
    /// The app-scoped default served to tenants with no theme of their own.
    #[must_use]
    pub fn shared() -> Self {
        Self {
            theme: "r2e-default".to_string(),
        }
    }
}

/// `TenantSource<Branding>` returning `Ok(None)` → the shared bean.
#[derive(Clone, Default)]
pub struct Brandings;

impl TenantSource<Branding> for Brandings {
    fn create<'a>(
        &'a self,
        tenant: &'a TenantId,
        ctx: &'a TenantContext<'a>,
    ) -> BoxFuture<'a, Result<Option<Branding>, BoxError>> {
        Box::pin(async move {
            let master = ctx.bean::<MasterDb>().ok_or("MasterDb not provided")?;
            Ok(master
                .theme(tenant.as_str())
                .await?
                .map(|theme| Branding { theme }))
        })
    }
}

// ── The Diesel half: a schema and a row to insert ───────────────────────────

diesel::table! {
    orders (id) {
        id -> Integer,
        name -> Text,
    }
}

/// The row the Diesel snippet inserts.
#[derive(Insertable)]
#[diesel(table_name = orders)]
pub struct NewOrder {
    pub name: &'static str,
}

/// The `&new` the Diesel snippet passes to `.values(..)`.
#[allow(non_upper_case_globals)]
pub static new: NewOrder = NewOrder { name: "Ada" };
