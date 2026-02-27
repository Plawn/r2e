use std::sync::Arc;
use std::time::Duration;

use jsonwebtoken::{encode, Algorithm, DecodingKey, EncodingKey, Header};
use r2e::prelude::*;
use r2e::r2e_openapi::{OpenApiConfig, OpenApiPlugin};
use r2e::r2e_observability::{Observability, ObservabilityConfig};
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

// --- Setup: runs once, persists across hot-patches ---

#[derive(Clone)]
struct AppEnv {
    config: R2eConfig,
    event_bus: EventBus,
    claims_validator: Arc<JwtClaimsValidator>,
    pool: sqlx::Pool<sqlx::Sqlite>,
    sse_broadcaster: r2e::sse::SseBroadcaster,
    notification_service: NotificationService,
    listener: Arc<std::net::TcpListener>,
}

async fn setup() -> AppEnv {
    let secret = b"r2e-demo-secret-change-in-production";

    // Print a test JWT for curl usage
    let token = generate_test_token(secret);
    println!("=== Test JWT (valid 1h) ===");
    println!("{token}");
    println!();

    let config = R2eConfig::load("dev").unwrap_or_else(|_| R2eConfig::empty());
    let event_bus = EventBus::new();

    let sec_config = SecurityConfig::new("unused", "r2e-demo", "r2e-app")
        .with_allowed_algorithm(jsonwebtoken::Algorithm::HS256);
    let claims_validator = JwtClaimsValidator::new_with_static_key(DecodingKey::from_secret(secret), sec_config);

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

    let sse_broadcaster = r2e::sse::SseBroadcaster::new(128);
    let notification_service = NotificationService::new(64);

    // Bind the listener ONCE in setup — survives across hot-patches via Arc.
    let listener = std::net::TcpListener::bind("0.0.0.0:3001").unwrap();
    listener.set_nonblocking(true).unwrap();

    AppEnv {
        config,
        event_bus,
        claims_validator: Arc::new(claims_validator),
        pool,
        sse_broadcaster,
        notification_service,
        listener: Arc::new(listener),
    }
}

// --- Server: hot-patched on every code change when dev-reload is enabled ---

#[r2e::main]
async fn main(env: AppEnv) {
    AppBuilder::new()
        .plugin(Scheduler)
        .provide(env.event_bus)
        .provide(env.pool)
        .provide(env.config.clone())
        .provide(env.claims_validator)
        .provide(r2e::r2e_rate_limit::RateLimitRegistry::default())
        .provide(env.sse_broadcaster)
        .provide(env.notification_service)
        .with_bean::<UserService>()
        .build_state::<Services, _, _>()
        .await
        .with_config(env.config)
        .with(Health)
        .with(Prometheus::builder()
            .endpoint("/metrics")
            .namespace("r2e")
            .exclude_path("/health")
            .exclude_path("/metrics")
            .build())
        .with(RequestIdPlugin)
        .with(SecureHeaders::default())
        .with(Cors::permissive())
        .with(Observability::new(
            ObservabilityConfig::new("r2e-example")
                .with_service_version("0.1.0")
                .capture_header("x-request-id"),
        ))
        .with(ErrorHandling)
        .with_layer(tower_http::timeout::TimeoutLayer::with_status_code(
            r2e::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .with(DevReload)
        .with(OpenApiPlugin::new(
            OpenApiConfig::new("R2E Example API", "0.1.1")
                .with_description("Demo application showcasing all R2E features")
                .with_docs_ui(true),
        ))
        .on_start(|_state| async move {
            tracing::info!("R2E example-app startup hook executed");
            Ok(())
        })
        .on_stop(|| async {
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
        .with(NormalizePath)
        .prepare("0.0.0.0:3001")
        .run_with_listener(
            tokio::net::TcpListener::from_std(env.listener.try_clone().unwrap()).unwrap()
        )
        .await
        .inspect_err(|e| eprintln!("=== SERVE ERROR: {e} ==="))
        .unwrap();
}
