//! Order management microservice (port 3002).

use r2e::prelude::*;
use r2e::r2e_openapi::{OpenApiConfig, OpenApiPlugin};

pub mod controllers;
pub mod models;
pub mod services;

use controllers::order_controller::OrderController;

/// The canonical Order Service application blueprint.
pub struct OrderApp;

impl App for OrderApp {
    type Env = ();

    async fn setup() {}

    async fn build(b: AppBuilder, _env: ()) -> impl BootableApp {
        // `serve_auto` (called by `launch`) reads `server.port` (3002) and the
        // `services.product.url` used by `ProductClient` from this service's
        // own config file.
        b.with_config_file("application-order.yaml")
            .load_config::<()>()
            .register::<services::ProductClient>()
            .register::<services::OrderService>()
            .build_state()
            .await
            .with(Health)
            .with(Cors::permissive())
            .with(Tracing)
            .with(ErrorHandling)
            .with(OpenApiPlugin::new(
                OpenApiConfig::new("Order Service", "1.0.0")
                    .with_description("Order management microservice")
                    .with_docs_ui(true),
            ))
            .register_controller::<OrderController>()
    }
}
