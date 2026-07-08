use r2e::prelude::*;
use r2e::r2e_openapi::{OpenApiConfig, OpenApiPlugin};

#[path = "../shared/mod.rs"]
mod shared;

#[path = "controllers/mod.rs"]
mod controllers;
#[path = "models.rs"]
mod models;
#[path = "services/mod.rs"]
mod services;

use controllers::product_controller::ProductController;

#[r2e::main]
async fn main() {
    let config = R2eConfig::load().unwrap_or_else(|_| R2eConfig::empty());

    AppBuilder::new()
        .with_config(config)
        .register::<services::ProductService>()
        .build_state()
        .await
        .with(Health)
        .with(Cors::permissive())
        .with(Tracing)
        .with(ErrorHandling)
        .with(OpenApiPlugin::new(
            OpenApiConfig::new("Product Service", "1.0.0")
                .with_description("Product catalog microservice")
                .with_docs_ui(true),
        ))
        .register_controller::<ProductController>()
        .serve("0.0.0.0:3001")
        .await
        .unwrap();
}
