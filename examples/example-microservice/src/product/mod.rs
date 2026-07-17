//! Product catalog microservice (port 3001).

use r2e::prelude::*;
use r2e::r2e_openapi::{OpenApiConfig, OpenApiPlugin};

pub mod controllers;
pub mod models;
pub mod services;

use controllers::product_controller::ProductController;

/// The canonical Product Service application blueprint.
pub struct ProductApp;

impl App for ProductApp {
    type Env = ();

    async fn setup() {}

    async fn build(b: AppBuilder, _env: ()) -> impl BootableApp {
        // `serve_auto` (called by `launch`) reads `server.port` (3001) from
        // this service's own config file.
        b.with_config_file("application-product.yaml")
            .load_config::<()>()
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
    }
}
