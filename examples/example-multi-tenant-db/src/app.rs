// Canonical example-multi-tenant-db application source.
//
// `lib.rs` includes this file so integration tests can boot `MultiTenantDbApp`;
// `app_main!` includes the same file directly in the binary tip crate.

use std::time::Duration;

use r2e::prelude::*;
use r2e::r2e_data_sqlx::PoolSource;
use r2e::tenant::{PerTenant, Tenancy};
use sqlx::{Pool, Sqlite};

pub mod controllers;
pub mod directory;
pub mod tenancy;

use controllers::{
    AdminController, ApiClientController, BrandingController, NotesController, WhoAmIController,
};
use directory::MasterDb;
use tenancy::{ApiClient, ApiClients, Branding, Brandings, HeaderResolver};

/// Provisioned once by [`App::setup`]: the master directory (and, behind it,
/// one SQLite file per tenant).
#[derive(Clone)]
pub struct AppEnv {
    /// The tenant directory — `tenants(slug, dsn, api_token, theme)`.
    pub master: MasterDb,
}

/// The canonical application blueprint.
pub struct MultiTenantDbApp;

impl App for MultiTenantDbApp {
    type Env = AppEnv;

    async fn setup() -> AppEnv {
        let dir = directory::data_dir();
        let master = directory::provision(&dir).await;

        println!("=== example-multi-tenant-db ===");
        println!("data directory: {}", dir.display());
        println!("tenants: acme (custom branding), globex (default branding)");
        println!();
        println!("  curl -H 'x-tenant-id: acme'   localhost:3000/notes");
        println!("  curl -H 'x-tenant-id: globex' localhost:3000/notes");
        println!("  curl localhost:3000/notes                      # 400, no tenant");
        println!("  curl -H 'x-tenant-id: ghost'  localhost:3000/notes   # 404, unknown tenant");
        println!("  curl localhost:3000/admin/pools");
        println!();

        AppEnv { master }
    }

    async fn build(b: AppBuilder, env: AppEnv) -> impl BootableApp {
        // The DSN lookup: one closure over the master directory. Its three
        // answers are the three the SPI defines — `Ok(Some(dsn))` provisioned,
        // `Ok(None)` unknown tenant (404, negatively cached), `Err` directory
        // unreachable (503, *not* cached, retried on the next request).
        let master = env.master.clone();
        let pool_source = PoolSource::<Sqlite>::new(move |tenant| {
            let master = master.clone();
            async move { Ok(master.dsn(tenant.as_str()).await?) }
        })
        // Per tenant! The steady-state connection count is this times the
        // number of live tenants. `max_active` below is a soft trim target, not
        // an admission bound for cold bursts.
        .max_connections(2);

        b.load_config::<()>()
            .provide(env.master)
            // ── 1. how a request names its tenant ──
            .provide(HeaderResolver)
            .plugin(Tenancy::resolver::<HeaderResolver>())
            // ── 2. how a tenant gets its database pool ──
            .provide(pool_source)
            .plugin(
                PerTenant::<Pool<Sqlite>>::from::<PoolSource<Sqlite>>()
                    // At most 16 live tenant pools × 2 connections = 32.
                    .max_active(16)
                    .idle_ttl(Duration::from_secs(300)),
            )
            // ── 3. cascade: a client built on the tenant's own pool ──
            .provide(ApiClients)
            .plugin(PerTenant::<ApiClient>::from::<ApiClients>().max_active(16))
            // ── 4. fallback: tenants without custom branding get the shared
            //       app-scoped bean instead of a 404. `.fallback_to_default()`
            //       adds `Branding` itself to the plugin's `Deps`, so the
            //       `.provide` above it is compile-checked, not hoped for. ──
            .provide(Branding::shared())
            .provide(Brandings)
            .plugin(PerTenant::<Branding>::from::<Brandings>().fallback_to_default())
            .build_state()
            .await
            .with(Health)
            .with(Tracing)
            .with(ErrorHandling)
            .register_controllers::<(
                NotesController,
                WhoAmIController,
                ApiClientController,
                BrandingController,
                AdminController,
            )>()
    }
}
