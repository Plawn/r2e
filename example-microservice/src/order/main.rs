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
#[path = "state.rs"]
mod state;

use controllers::order_controller::OrderController;
use state::OrderState;

#[tokio::main]
async fn main() {
    r2e::init_tracing();

    let config = R2eConfig::load("order").unwrap_or_else(|_| R2eConfig::empty());

    AppBuilder::new()
        .provide(config.clone())
        .with_bean::<services::ProductClient>()
        .with_bean::<services::OrderService>()
        .build_state::<OrderState, _>()
        .await
        .with_config(config)
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
        .serve("0.0.0.0:3002")
        .await
        .unwrap();
}
