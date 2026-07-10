//! Example application library.
//!
//! Everything except `main()` lives here so that integration tests can boot
//! the **same** application via the blueprint ([`app`]) instead of
//! copy-pasting controllers and services:
//!
//! ```ignore
//! #[r2e::test(app = example_app::app)]
//! async fn lists_users(app: TestApp) {
//!     app.get("/users").as_user("alice", &["user"]).send().await.assert_ok();
//! }
//! ```

use std::sync::Arc;
use std::time::Duration;

use jsonwebtoken::{encode, Algorithm, DecodingKey, EncodingKey, Header};
use r2e::prelude::*;
use r2e::r2e_observability::{Observability, ObservabilityConfig};
use r2e::r2e_openapi::{OpenApiConfig, OpenApiPlugin};
use r2e::r2e_prometheus::Prometheus;
use r2e::r2e_scheduler::Scheduler;
use r2e::r2e_security::{JwtClaimsValidator, SecurityConfig};
use sqlx::SqlitePool;

pub mod controllers;
pub mod db_identity;
pub mod error;
pub mod models;
pub mod services;

use controllers::account_controller::AccountController;
use controllers::config_controller::ConfigController;
use controllers::data_controller::DataController;
use controllers::db_identity_controller::IdentityController;
use controllers::event_controller::UserEventConsumer;
use controllers::mixed_controller::MixedController;
use controllers::notification_controller::NotificationController;
use controllers::scheduled_controller::ScheduledJobs;
use controllers::sse_controller::SseController;
use controllers::upload_controller::UploadController;
use controllers::user_controller::UserController;
use controllers::ws_controller::WsEchoController;
use services::{NotificationService, UserService};

/// The "users" vertical slice as a feature module: one `register_module`
/// call registers the service and both controllers. `UserService` is
/// exported (other controllers inject it); the imports are satisfied by the
/// app's `.provide`/`.load_config` calls below. Decorator deps count too:
/// `UserController`'s rate-limit guard and cache interceptor read
/// `RateLimitRegistry` and the cache store bean, so the module imports them.
#[module(
    providers(UserService),
    controllers(UserController, UserEventConsumer),
    exports(UserService),
    imports(
        LocalEventBus,
        sqlx::SqlitePool,
        R2eConfig,
        r2e::r2e_rate_limit::RateLimitRegistry,
        std::sync::Arc<dyn r2e::r2e_cache::CacheStore>,
    )
)]
pub struct UserModule;

const DEMO_SECRET: &[u8] = b"r2e-demo-secret-change-in-production";

/// Generate a demo JWT accepted by the app's own validator (for curl usage).
pub fn demo_token() -> String {
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
    encode(&header, &claims, &EncodingKey::from_secret(DEMO_SECRET)).unwrap()
}

/// Resources provisioned once. In dev mode they survive hot patches
/// (`main.rs` keeps the `AppEnv` alive across code reloads); the blueprint
/// provisions a fresh set per boot.
#[derive(Clone)]
pub struct AppEnv {
    event_bus: LocalEventBus,
    claims_validator: Arc<JwtClaimsValidator>,
    pool: sqlx::Pool<sqlx::Sqlite>,
    sse_broadcaster: r2e::sse::SseBroadcaster,
    notification_service: NotificationService,
}

/// Provision the app's external resources: in-memory SQLite with seed data,
/// event bus, JWT validator, SSE broadcaster.
pub async fn setup() -> AppEnv {
    let event_bus = LocalEventBus::new();

    let sec_config = SecurityConfig::new("unused", "r2e-demo", "r2e-app")
        .with_allowed_algorithm(jsonwebtoken::Algorithm::HS256);
    let claims_validator =
        JwtClaimsValidator::new_with_static_key(DecodingKey::from_secret(DEMO_SECRET), sec_config);

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

    AppEnv {
        event_bus,
        claims_validator: Arc::new(claims_validator),
        pool,
        sse_broadcaster,
        notification_service,
    }
}

/// Application blueprint: provision fresh resources and assemble the full
/// app. Tests boot this via `TestApp::boot(example_app::app)` or
/// `#[r2e::test(app = example_app::app)]`.
pub async fn app(b: AppBuilder) -> impl BootableApp {
    let env = setup().await;
    app_with_env(b, env).await
}

/// Assemble the app on pre-provisioned resources. `main.rs` calls this
/// directly so the dev hot-reload path can keep [`AppEnv`] alive across
/// patches.
pub async fn app_with_env(b: AppBuilder, env: AppEnv) -> impl BootableApp {
    b.plugin(Scheduler)
        .plugin(
            Prometheus::builder()
                .endpoint("/metrics")
                .namespace("r2e")
                .exclude_path("/health")
                .exclude_path("/metrics")
                .build(),
        )
        .load_config::<controllers::config_controller::RootConfig>()
        .provide(env.event_bus)
        .provide(env.pool)
        .provide(env.claims_validator)
        .provide(r2e::r2e_rate_limit::RateLimitRegistry::default())
        .provide(r2e::r2e_cache::InMemoryStore::shared())
        .provide(env.sse_broadcaster)
        .provide(SseTopic::<models::UserCreatedEvent>::new(64).with_event_name("user_created"))
        .provide(env.notification_service)
        .register_module::<UserModule>()
        .build_state()
        .await
        // EventBus↔SSE bridge: every UserCreatedEvent emitted on the bus is
        // broadcast on the topic (served at /sse/users) with no liaison code.
        .bridge_sse::<LocalEventBus, models::UserCreatedEvent>()
        .with(Health)
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
        .with(OpenApiPlugin::new(
            OpenApiConfig::new("R2E Example API", "0.1.1")
                .with_description("Demo application showcasing all R2E features")
                .with_docs_ui(true),
        ))
        .on_start(|_state| async move {
            tracing::info!("R2E example-app startup hook executed");
            Ok(())
        })
        .on_stop(|_| async {
            tracing::info!("R2E example-app shutdown hook executed");
        })
        .register_controllers::<(
            AccountController,
            ConfigController,
            DataController,
            MixedController,
            IdentityController,
            ScheduledJobs,
            SseController,
            WsEchoController,
            NotificationController,
            UploadController,
        )>()
        .with(NormalizePath)
}
