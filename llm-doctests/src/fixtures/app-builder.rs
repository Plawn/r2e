//! Scaffolding for `llm/app-builder.md`.

use r2e::prelude::*;

pub use sqlx::SqlitePool;

/// Neither plugin is in the prelude: `use r2e::r2e_executor::Executor;` and
/// `use r2e::r2e_openapi::{OpenApiConfig, OpenApiPlugin};`.
pub use r2e::r2e_executor::Executor;
pub use r2e::r2e_openapi::{OpenApiConfig, OpenApiPlugin};

/// The app's root config type — `load_config::<RootConfig>()`.
#[derive(ConfigProperties, Clone, Debug)]
pub struct RootConfig {
    pub greeting: Option<String>,
}

/// An already-constructed value handed to `.provide(bean)`.
#[derive(Clone)]
pub struct Metrics;

#[allow(non_upper_case_globals)]
pub const bean: Metrics = Metrics;

/// A process-lifetime resource built in `App::setup`.
#[derive(Clone)]
pub struct S3Client;

/// The `App::Env` bundle handed to `.provide_all(env)`.
#[derive(ProvideBundle)]
pub struct AppEnv {
    pub s3: S3Client,
}

#[allow(non_upper_case_globals)]
pub const env: AppEnv = AppEnv { s3: S3Client };

/// Generates `struct CreatePool`, registered with `.register::<CreatePool>()`.
#[producer]
pub async fn create_pool(#[config("database.url")] url: String) -> Result<SqlitePool, sqlx::Error> {
    SqlitePool::connect(&url).await
}

/// A service a feature module provides.
#[derive(Clone)]
pub struct UserService;

#[bean]
impl UserService {
    pub fn new() -> Self {
        Self
    }
}

/// The slice registered with `.register_module::<UserModule>()`.
#[module(providers(UserService), exports(UserService))]
pub struct UserModule;

#[derive(Clone)]
pub struct BillingService;

#[bean]
impl BillingService {
    pub fn new() -> Self {
        Self
    }
}

#[module(providers(BillingService), exports(BillingService))]
pub struct BillingModule;

/// The aggregate registered with `.register_modules::<AppModules>()`.
#[module(modules(BillingModule))]
pub struct AppModules;

/// The controllers the app phase registers.
#[controller(path = "/users")]
pub struct UserController {
    #[inject]
    user_service: UserService,
}

#[routes]
impl UserController {
    #[get("/")]
    async fn list(&self) -> Json<Vec<String>> {
        let _ = &self.user_service;
        Json(Vec::new())
    }
}

#[controller(path = "/accounts")]
pub struct AccountController;

#[routes]
impl AccountController {
    #[get("/")]
    async fn list(&self) -> Json<Vec<String>> {
        Json(Vec::new())
    }
}

#[controller]
pub struct ScheduledJobs;

#[routes]
impl ScheduledJobs {
    #[scheduled(every = 30)]
    async fn count_users(&self) {}
}
