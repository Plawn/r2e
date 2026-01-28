use std::sync::Arc;
use std::time::Duration;

use jsonwebtoken::{encode, Algorithm, DecodingKey, EncodingKey, Header};
use quarlus_core::config::{ConfigValue, QuarlusConfig};
use quarlus_core::AppBuilder;
use quarlus_events::EventBus;
use quarlus_openapi::{AppBuilderOpenApiExt, OpenApiConfig};
use quarlus_scheduler::AppBuilderSchedulerExt;
use quarlus_security::{JwtValidator, SecurityConfig};
use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;

mod controllers;
mod models;
mod services;
mod state;

use controllers::account_controller::AccountController;
use controllers::config_controller::ConfigController;
use controllers::data_controller::DataController;
use controllers::event_controller::UserEventConsumer;
use controllers::mixed_controller::MixedController;
use controllers::scheduled_controller::ScheduledJobs;
use controllers::user_controller::UserController;
use services::UserService;
use state::Services;

fn generate_test_token(secret: &[u8]) -> String {
    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;

    let claims = serde_json::json!({
        "sub": "user-123",
        "email": "demo@quarlus.dev",
        "roles": ["user", "admin"],
        "iss": "quarlus-demo",
        "aud": "quarlus-app",
        "exp": exp,
    });

    let header = Header::new(Algorithm::HS256);
    encode(&header, &claims, &EncodingKey::from_secret(secret)).unwrap()
}

#[tokio::main]
async fn main() {
    quarlus_core::init_tracing();

    let secret = b"quarlus-demo-secret-change-in-production";

    // Print a test JWT for curl usage
    let token = generate_test_token(secret);
    println!("=== Test JWT (valid 1h) ===");
    println!("{token}");
    println!();

    // --- Configuration (#1) ---
    // load() succeeds even when application.yaml is absent (env vars still overlay),
    // so we always ensure the required keys exist with sensible defaults.
    let mut config = QuarlusConfig::load("dev").unwrap_or_else(|_| QuarlusConfig::empty());
    if config.get::<String>("app.name").is_err() {
        config.set("app.name", ConfigValue::String("Quarlus Example App".into()));
    }
    if config.get::<String>("app.greeting").is_err() {
        config.set("app.greeting", ConfigValue::String("Welcome to Quarlus!".into()));
    }
    if config.get::<String>("app.version").is_err() {
        config.set("app.version", ConfigValue::String("0.1.0".into()));
    }

    // --- Events (#7) ---
    let event_bus = EventBus::new();

    // Build the JWT validator with a static HMAC key (no JWKS needed for the demo)
    let sec_config = SecurityConfig::new("unused", "quarlus-demo", "quarlus-app");
    let validator =
        JwtValidator::new_with_static_key(DecodingKey::from_secret(secret), sec_config);

    // Create an in-memory SQLite pool and initialise the schema
    let pool: sqlx::Pool<sqlx::Sqlite> = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS users (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            email TEXT NOT NULL
        )",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Seed database with sample users for data controller (#6)
    for (name, email) in [
        ("Alice", "alice@example.com"),
        ("Bob", "bob@example.com"),
        ("Charlie", "charlie@example.com"),
    ] {
        sqlx::query("INSERT INTO users (name, email) VALUES (?, ?)")
            .bind(name)
            .bind(email)
            .execute(&pool)
            .await
            .unwrap();
    }
    // --- Scheduling (#8) is now declarative via #[scheduled] in ScheduledJobs ---
    let cancel = CancellationToken::new();

    let services = Services {
        user_service: UserService::new(event_bus.clone()),
        jwt_validator: Arc::new(validator),
        pool,
        event_bus,
        config: config.clone(),
        cancel: cancel.clone(),
        rate_limiter: quarlus_rate_limit::RateLimitRegistry::default(),
    };

    // --- App assembly ---
    AppBuilder::new()
        .with_state(services)
        .with_config(config)
        .with_health()
        .with_cors()
        .with_tracing()
        .with_error_handling() // Error handling (#3)
        .with_layer(tower_http::timeout::TimeoutLayer::with_status_code(Duration::from_secs(30), axum::http::StatusCode::REQUEST_TIMEOUT)) // Global Tower layer
        .with_dev_reload() // Dev mode (#9)
        .with_scheduler(|s| {
            s.register::<ScheduledJobs>(); // Scheduling (#8)
        })
        .with_openapi(
            OpenApiConfig::new("Quarlus Example API", "0.1.0")
                .with_description("Demo application showcasing all Quarlus features")
                .with_docs_ui(true),
        ) // OpenAPI (#5)
        .on_start(|_state| async move {
            // Lifecycle hook (#10)
            tracing::info!("Quarlus example-app startup hook executed");
            Ok(())
        })
        .on_stop(|| async {
            // Lifecycle hook (#10)
            tracing::info!("Quarlus example-app shutdown hook executed");
        })
        .register_controller::<UserController>()
        .register_controller::<AccountController>()
        .register_controller::<ConfigController>()
        .register_controller::<DataController>()
        .register_controller::<UserEventConsumer>()
        .register_controller::<MixedController>()
        .serve("0.0.0.0:3000")
        .await
        .unwrap();
}
