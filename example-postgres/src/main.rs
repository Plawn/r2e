use r2e::prelude::*;
use r2e::r2e_openapi::{OpenApiConfig, OpenApiPlugin};

mod controllers;
mod error;
mod models;
mod services;
mod state;

use controllers::article_controller::ArticleController;
use state::AppState;

#[producer]
async fn create_pool(#[config("database.url")] url: String) -> sqlx::PgPool {
    sqlx::PgPool::connect(&url)
        .await
        .expect("Failed to connect to PostgreSQL")
}

#[tokio::main]
async fn main() {
    r2e::init_tracing();

    let config = R2eConfig::load("dev").unwrap_or_else(|_| R2eConfig::empty());

    AppBuilder::new()
        .provide(config.clone())
        .with_producer::<CreatePool>()
        .with_bean::<services::ArticleService>()
        .build_state::<AppState, _>()
        .await
        .with_config(config)
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
            sqlx::migrate!("./migrations")
                .run(&state.pool)
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            tracing::info!("Database migrations applied");
            Ok(())
        })
        .register_controller::<ArticleController>()
        .serve("0.0.0.0:3000")
        .await
        .unwrap();
}
