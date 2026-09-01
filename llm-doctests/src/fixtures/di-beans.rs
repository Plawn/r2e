//! Scaffolding for `llm/di-beans.md`.

use r2e::prelude::*;

pub use std::sync::Arc;

/// The raw sqlx pools the producer snippets hand to the graph.
pub use sqlx::{PgPool, SqlitePool};

/// The `Executor` plugin is not in the prelude (`use r2e::r2e_executor::Executor;`).
pub use r2e::r2e_executor::Executor;

/// The app's root config type passed to `load_config::<RootConfig>()`.
#[derive(ConfigProperties, Clone, Debug)]
pub struct RootConfig {
    pub greeting: Option<String>,
}

/// Stand-in for the JWT validator built in `App::setup` and provided as a bean.
#[derive(Clone)]
pub struct ClaimsValidator;

#[allow(non_upper_case_globals)]
pub const claims_validator: ClaimsValidator = ClaimsValidator;

/// The service the assembly snippet registers.
#[derive(Clone)]
pub struct UserService {
    pub pool: SqlitePool,
    pub event_bus: LocalEventBus,
}

#[bean]
impl UserService {
    pub fn new(pool: SqlitePool, event_bus: LocalEventBus) -> Self {
        Self { pool, event_bus }
    }
}

/// Generates `struct CreatePool`, registered with `.register::<CreatePool>()`.
#[producer]
pub async fn create_pool(#[config("database.url")] url: String) -> Result<SqlitePool, sqlx::Error> {
    SqlitePool::connect(&url).await
}

/// A database handle a `#[producer]` opens from a URL.
#[derive(Clone)]
pub struct Db;

impl Db {
    pub fn open(_url: &str) -> Self {
        Self
    }
}

/// Ordering-only dependencies for `#[producer(after(..))]`.
#[derive(Clone)]
pub struct InstanceGuard;

#[bean]
impl InstanceGuard {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Clone)]
pub struct Migrations;

#[bean]
impl Migrations {
    pub fn new() -> Self {
        Self
    }
}

/// A migration helper generic over sqlx's `Acquire`, as in the `!Send` section.
pub async fn run_migration_step<'a, A: sqlx::Acquire<'a>>(_conn: A) {}

/// A process-lifetime resource of `App::Env` that is only sometimes configured.
#[derive(Clone)]
pub struct S3Client;

/// The optional client the `Option<T>` bean section produces.
pub struct LlmClient;

impl LlmClient {
    pub fn new(_api_key: &str) -> Self {
        Self
    }
}
