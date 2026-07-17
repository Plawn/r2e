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

#[producer]
async fn create_pool(#[config("database.url")] url: String) -> sqlx::PgPool {
    sqlx::PgPool::connect(&url)
        .await
        .expect("Failed to connect to PostgreSQL")
}

/// The canonical application blueprint.
pub struct PostgresApp;

impl App for PostgresApp {
    type Env = ();

    async fn setup() {}

    async fn build(b: AppBuilder, _env: ()) -> impl BootableApp {
        b.load_config::<()>()
            .register::<CreatePool>()
            .register::<services::ArticleService>()
            .build_state()
            .await
            .with(Health)
            .with(Cors::permissive())
            .with(Tracing)
            .with(ErrorHandling)
            .with(OpenApiPlugin::new(
                OpenApiConfig::new("Articles API", "1.0.0")
                    .with_description("PostgreSQL CRUD example")
                    .with_docs_ui(true),
            ))
            .on_start(|state| async move {
                // Run migrations
                let pool = state
                    .bean::<sqlx::PgPool>()
                    .expect("PgPool bean not found in state");
                sqlx::migrate!("./migrations")
                    .run(&pool)
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
                tracing::info!("Database migrations applied");
                Ok(())
            })
            .register_controller::<ArticleController>()
    }
}
