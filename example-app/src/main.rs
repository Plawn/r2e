use std::sync::Arc;
use std::time::Duration;

use jsonwebtoken::{encode, Algorithm, DecodingKey, EncodingKey, Header};
use r2e::prelude::*;
use r2e::r2e_openapi::{OpenApiConfig, OpenApiPlugin};
use r2e::r2e_prometheus::Prometheus;
use r2e::r2e_scheduler::Scheduler;
use r2e::r2e_security::{JwtClaimsValidator, SecurityConfig};
use sqlx::SqlitePool;

mod controllers;
mod db_identity;
mod models;
mod services;
mod state;

use controllers::account_controller::AccountController;
use controllers::config_controller::ConfigController;
use controllers::data_controller::DataController;
use controllers::db_identity_controller::IdentityController;
use controllers::event_controller::UserEventConsumer;
use controllers::mixed_controller::MixedController;
use controllers::scheduled_controller::ScheduledJobs;
use controllers::sse_controller::SseController;
use controllers::user_controller::UserController;
use controllers::notification_controller::NotificationController;
use controllers::upload_controller::UploadController;
use controllers::ws_controller::WsEchoController;
use services::{NotificationService, UserService};
use state::Services;

fn generate_test_token(secret: &[u8]) -> String {
    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;

    let claims = serde_json::json!({
        "sub": "user-123",
        "email": "demo@r2e.dev",
        "roles": ["user", "admin"],
        "iss": "r2e-demo",
        "aud": "r2e-app",
        "exp": exp,
    });

    let header = Header::new(Algorithm::HS256);
    encode(&header, &claims, &EncodingKey::from_secret(secret)).unwrap()
}

#[tokio::main]
async fn main() {
    r2e::init_tracing();

    let secret = b"r2e-demo-secret-change-in-production";

    // Print a test JWT for curl usage
    let token = generate_test_token(secret);
    println!("=== Test JWT (valid 1h) ===");
    println!("{token}");
    println!();

    // --- Configuration (#1) ---
    // load() succeeds even when application.yaml is absent (env vars still overlay),
    // so we always ensure the required keys exist with sensible defaults.
    let config = R2eConfig::load("dev").unwrap_or_else(|_| R2eConfig::empty());

    // --- Events (#7) ---
    let event_bus = EventBus::new();

    // Build the JWT claims validator with a static HMAC key (no JWKS needed for the demo)
    // Using JwtClaimsValidator allows multiple identity types (AuthenticatedUser, DbUser, etc.)
    let sec_config = SecurityConfig::new("unused", "r2e-demo", "r2e-app");
    let claims_validator = JwtClaimsValidator::new_with_static_key(DecodingKey::from_secret(secret), sec_config);

    // Create an in-memory SQLite pool and initialise the schema
    let pool: sqlx::Pool<sqlx::Sqlite> = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS users (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            email TEXT NOT NULL,
            sub TEXT
        )",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Seed database with sample users for data controller (#6)
    // The first user has a `sub` matching the demo JWT token ("user-123"),
    // allowing the DbIdentityController to resolve it from the database.
    for (name, email, sub) in [
        ("Alice", "alice@example.com", Some("user-123")),
        ("Bob", "bob@example.com", None),
        ("Charlie", "charlie@example.com", None),
    ] {
        sqlx::query("INSERT INTO users (name, email, sub) VALUES (?, ?, ?)")
            .bind(name)
            .bind(email)
            .bind(sub)
            .execute(&pool)
            .await
            .unwrap();
    }
    // --- App assembly using bean graph ---
    // Scheduler (#8) is installed before build_state() to provide CancellationToken
    // SSE broadcaster for real-time events
    let sse_broadcaster = r2e::sse::SseBroadcaster::new(128);
    let notification_service = NotificationService::new(64);

    AppBuilder::new()
        .plugin(Scheduler) // Scheduling (#8) - provides CancellationToken
        .provide(event_bus)
        .provide(pool)
        .provide(config.clone())
        .provide(Arc::new(claims_validator))
        .provide(r2e::r2e_rate_limit::RateLimitRegistry::default())
        .provide(sse_broadcaster)
        .provide(notification_service)
        .with_bean::<UserService>()
        .build_state::<Services, _>()
        .with_config(config)
        .with(Health)
        .with(Prometheus::builder()
            .endpoint("/metrics")
            .namespace("r2e")
            .exclude_path("/health")
            .exclude_path("/metrics")
            .build())
        .with(RequestIdPlugin)  // Request ID propagation
        .with(SecureHeaders::default()) // Security headers
        .with(Cors::permissive())
        .with(Tracing)
        .with(ErrorHandling) // Error handling (#3)
        .with_layer(tower_http::timeout::TimeoutLayer::with_status_code(
            r2e::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        )) // Global Tower layer
        .with(DevReload) // Dev mode (#9)
        .with(OpenApiPlugin::new(
            OpenApiConfig::new("R2E Example API", "0.1.0")
                .with_description("Demo application showcasing all R2E features")
                .with_docs_ui(true),
        )) // OpenAPI (#5)
        .on_start(|_state| async move {
            // Lifecycle hook (#10)
            tracing::info!("R2E example-app startup hook executed");
            Ok(())
        })
        .on_stop(|| async {
            // Lifecycle hook (#10)
            tracing::info!("R2E example-app shutdown hook executed");
        })
        .register_controller::<UserController>()
        .register_controller::<AccountController>()
        .register_controller::<ConfigController>()
        .register_controller::<DataController>()
        .register_controller::<UserEventConsumer>()
        .register_controller::<MixedController>()
        .register_controller::<IdentityController>()
        .register_controller::<ScheduledJobs>()
        .register_controller::<SseController>()
        .register_controller::<WsEchoController>()
        .register_controller::<NotificationController>()
        .register_controller::<UploadController>()
        .with(NormalizePath) // Must be last to normalize paths before routing
        .serve("0.0.0.0:3001")
        .await
        .unwrap();
}
