//! Cold, process-lifetime resources for the example application.
//!
//! `r2e dev` performs a full restart when this file changes. That keeps a
//! previously allocated `AppEnv` from crossing an incompatible hot-patch.

use std::sync::Arc;

use jsonwebtoken::{encode, Algorithm, DecodingKey, EncodingKey, Header};
use r2e::prelude::*;
use r2e::r2e_security::{JwtClaimsValidator, SecurityConfig};
use sqlx::SqlitePool;

use crate::services::NotificationService;

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

/// Resources provisioned once by `App::setup` and retained across hot-patches.
#[derive(Clone)]
pub struct AppEnv {
    pub(crate) event_bus: LocalEventBus,
    pub(crate) claims_validator: Arc<JwtClaimsValidator>,
    pub(crate) pool: sqlx::Pool<sqlx::Sqlite>,
    pub(crate) sse_broadcaster: r2e::sse::SseBroadcaster,
    pub(crate) notification_service: NotificationService,
}

/// Provision the app's external resources: in-memory SQLite with seed data,
/// event bus, JWT validator, SSE broadcaster.
pub(crate) async fn provision_env() -> AppEnv {
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
