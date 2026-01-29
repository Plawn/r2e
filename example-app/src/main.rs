use std::sync::Arc;
use std::time::Duration;

use jsonwebtoken::{encode, Algorithm, DecodingKey, EncodingKey, Header};
use quarlus_core::config::QuarlusConfig;
use quarlus_core::plugins::{Cors, DevReload, ErrorHandling, Health, NormalizePath, Tracing};
use quarlus_core::AppBuilder;
use quarlus_events::EventBus;
use quarlus_openapi::{OpenApiConfig, OpenApiPlugin};
use quarlus_prometheus::Prometheus;
use quarlus_scheduler::Scheduler;
use quarlus_security::{JwtValidator, SecurityConfig};
use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;

mod controllers;
mod db_identity;
mod models;
mod services;
mod state;

use controllers::account_controller::AccountController;
use controllers::config_controller::ConfigController;
use controllers::data_controller::DataController;
use controllers::db_identity_controller::DbIdentityController;
use controllers::event_controller::UserEventConsumer;
use controllers::mixed_controller::MixedController;
use controllers::scheduled_controller::ScheduledJobs;
use controllers::user_controller::UserController;
use db_identity::DbIdentityBuilder;
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
    let config = QuarlusConfig::load("dev").unwrap_or_else(|_| QuarlusConfig::empty());

    // --- Events (#7) ---
    let event_bus = EventBus::new();

    // Build the JWT validator with a static HMAC key (no JWKS needed for the demo)
    let sec_config = SecurityConfig::new("unused", "quarlus-demo", "quarlus-app");
    let validator = JwtValidator::new_with_static_key(DecodingKey::from_secret(secret), sec_config);

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
    // Build a DB-backed JWT validator: same JWT verification, but the identity
    // is resolved from the database instead of raw claims. See db_identity.rs.
    let db_sec_config = SecurityConfig::new("unused", "quarlus-demo", "quarlus-app");
    let db_validator = JwtValidator::from_static_key(
        DecodingKey::from_secret(secret),
        db_sec_config,
        DbIdentityBuilder::new(pool.clone()),
    );

    // --- Scheduling (#8) is now declarative via #[scheduled] in ScheduledJobs ---
    let cancel = CancellationToken::new();

    // --- App assembly using bean graph ---
    AppBuilder::new()
        .provide(event_bus)
        .provide(pool)
        .provide(config.clone())
        .provide(Arc::new(validator))
        .provide(Arc::new(db_validator))
        .provide(cancel)
        .provide(quarlus_rate_limit::RateLimitRegistry::default())
        .with_bean::<UserService>()
        .build_state::<Services, _>()
        .with_config(config)
        .with(Health)
        .with(Prometheus::builder()
            .endpoint("/metrics")
            .namespace("quarlus")
            .exclude_path("/health")
            .exclude_path("/metrics")
            .build())
        .with(Cors::permissive())
        .with(Tracing)
        .with(ErrorHandling) // Error handling (#3)
        .with_layer(tower_http::timeout::TimeoutLayer::with_status_code(
            quarlus_core::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        )) // Global Tower layer
        .with(DevReload) // Dev mode (#9)
        .with(Scheduler) // Scheduling (#8)
        .with(OpenApiPlugin::new(
            OpenApiConfig::new("Quarlus Example API", "0.1.0")
                .with_description("Demo application showcasing all Quarlus features")
                .with_docs_ui(true),
        )) // OpenAPI (#5)
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
        .register_controller::<DbIdentityController>()
        .register_controller::<ScheduledJobs>()
        .with(NormalizePath) // Must be last to normalize paths before routing
        .serve("0.0.0.0:3001")
        .await
        .unwrap();
}
