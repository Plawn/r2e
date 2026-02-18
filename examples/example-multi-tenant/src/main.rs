use std::sync::Arc;

use jsonwebtoken::{encode, Algorithm, DecodingKey, EncodingKey, Header};
use r2e::prelude::*;
use r2e::r2e_openapi::{OpenApiConfig, OpenApiPlugin};
use r2e::r2e_security::{JwtClaimsValidator, SecurityConfig};
use sqlx::SqlitePool;

mod controllers;
mod models;
mod services;
mod state;
mod tenant_guard;
mod tenant_identity;

use controllers::admin_controller::AdminController;
use controllers::tenant_controller::TenantController;
use state::AppState;

fn generate_token(secret: &[u8], sub: &str, tenant_id: &str, roles: &[&str]) -> String {
    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;

    let claims = serde_json::json!({
        "sub": sub,
        "tenant_id": tenant_id,
        "roles": roles,
        "iss": "r2e-multi-tenant",
        "aud": "r2e-app",
        "exp": exp,
    });

    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret),
    )
    .unwrap()
}

#[tokio::main]
async fn main() {
    r2e::init_tracing();

    let secret = b"multi-tenant-secret-change-in-production";

    // Print test tokens for curl usage
    println!("=== Test JWTs (valid 1h) ===");
    println!(
        "Tenant acme (user):  {}",
        generate_token(secret, "alice", "acme", &["user"])
    );
    println!(
        "Tenant acme (admin): {}",
        generate_token(secret, "bob", "acme", &["user", "admin"])
    );
    println!(
        "Tenant globex (user): {}",
        generate_token(secret, "charlie", "globex", &["user"])
    );
    println!(
        "Super-admin:          {}",
        generate_token(secret, "root", "system", &["super-admin"])
    );
    println!();

    let config = R2eConfig::load("dev").unwrap_or_else(|_| R2eConfig::empty());

    let sec_config = SecurityConfig::new("unused", "r2e-multi-tenant", "r2e-app")
        .with_allowed_algorithm(jsonwebtoken::Algorithm::HS256);
    let claims_validator =
        JwtClaimsValidator::new_with_static_key(DecodingKey::from_secret(secret), sec_config);

    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS projects (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            tenant_id TEXT NOT NULL,
            name TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT ''
        )",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Seed sample data
    for (tenant, name, desc) in [
        ("acme", "Website Redesign", "Redesign the corporate website"),
        ("acme", "Mobile App", "iOS and Android app"),
        ("globex", "Data Pipeline", "ETL pipeline for analytics"),
    ] {
        sqlx::query("INSERT INTO projects (tenant_id, name, description) VALUES (?, ?, ?)")
            .bind(tenant)
            .bind(name)
            .bind(desc)
            .execute(&pool)
            .await
            .unwrap();
    }

    AppBuilder::new()
        .provide(pool)
        .provide(config.clone())
        .provide(Arc::new(claims_validator))
        .with_bean::<services::ProjectService>()
        .build_state::<AppState, _>()
        .await
        .with_config(config)
        .with(Health)
        .with(Cors::permissive())
        .with(Tracing)
        .with(ErrorHandling)
        .with(OpenApiPlugin::new(
            OpenApiConfig::new("Multi-Tenant API", "1.0.0")
                .with_description("Tenant isolation via JWT claims and custom guards")
                .with_docs_ui(true),
        ))
        .register_controller::<TenantController>()
        .register_controller::<AdminController>()
        .serve("0.0.0.0:3000")
        .await
        .unwrap();
}
