// Canonical example-postgres application source.
//
// `lib.rs` includes this file so the app can be booted by type; `app_main!`
// includes the same file directly in the binary tip crate for production and
// real Subsecond hot-patching.

use r2e::prelude::*;
use r2e::r2e_openapi::{OpenApiConfig, OpenApiPlugin};

pub mod controllers;
pub mod error;
pub mod models;
pub mod services;

use controllers::article_controller::ArticleController;

/// The app's schema, compiled into the binary. The datasource plugin applies
/// it at boot when `datasource.migrate-at-start` is true.
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// The canonical application blueprint.
pub struct PostgresApp;

impl App for PostgresApp {
    type Env = ();

    async fn setup() -> Result<(), BootError> {
        Ok(())
    }

    async fn build(b: AppBuilder, _env: ()) -> Result<impl BootableApp, BootError> {
        Ok({
        b.load_config::<()>()
            // Connects the pool from `datasource.*`, runs the migrations, and
            // closes the pool on shutdown — no producer, no `on_start` hook.
            .plugin(SqlxDataSource::<sqlx::Postgres>::new().migrations(&MIGRATOR))
            .register::<services::ArticleService>()
            .plugin(Health)
            .plugin(Cors::permissive())
            .plugin(HttpTrace::new())
            .plugin(OpenApiPlugin::new(
                OpenApiConfig::new("Articles API", "1.0.0")
                    .with_description("PostgreSQL CRUD example")
                    .with_docs_ui(true),
            ))
            .build_state()
            .await
            .register_controller::<ArticleController>()
    })
    }
}
