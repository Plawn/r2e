use std::sync::Arc;

use jsonwebtoken::{encode, Algorithm, DecodingKey, EncodingKey, Header};
use quarlus_core::AppBuilder;
use quarlus_security::{JwtValidator, SecurityConfig};
use sqlx::SqlitePool;

mod controllers;
mod models;
mod services;
mod state;

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

    // Build the JWT validator with a static HMAC key (no JWKS needed for the demo)
    let config = SecurityConfig::new("unused", "quarlus-demo", "quarlus-app");
    let validator = JwtValidator::new_with_static_key(DecodingKey::from_secret(secret), config);

    // Create an in-memory SQLite pool and initialise the schema
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
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

    let services = Services {
        user_service: UserService::new(),
        jwt_validator: Arc::new(validator),
        pool,
    };

    AppBuilder::new()
        .with_state(services)
        .with_health()
        .with_cors()
        .with_tracing()
        .register_controller::<UserController>()
        .serve("0.0.0.0:3000")
        .await
        .unwrap();
}
