//! Scaffolding for `llm/plugins.md`.

use r2e::prelude::*;

/// The concrete bean the example plugins take as a dependency.
pub use sqlx::SqlitePool;

/// Not in the prelude: `use r2e::builtins::health::{HealthIndicator,
/// HealthRegistry, HealthStatus};`, `use r2e::plugin::DeferredContext;` and
/// `use r2e::rt;`.
pub use r2e::builtins::health::{HealthIndicator, HealthRegistry, HealthStatus};
pub use r2e::plugin::DeferredContext;
pub use r2e::rt;

/// The bean `MyPlugin` provides.
#[derive(Clone)]
pub struct MyHandle;

impl MyHandle {
    pub async fn connect(_pool: SqlitePool) -> Result<Self, PluginBuildError> {
        Ok(Self)
    }

    pub async fn drain(&self) {}
}

/// The probe the `HealthRegistry` snippet contributes.
pub struct PingIndicator {
    pub pool: SqlitePool,
}

impl HealthIndicator for PingIndicator {
    fn name(&self) -> &str {
        "db"
    }

    async fn check(&self) -> HealthStatus {
        HealthStatus::Up
    }
}

/// The serve-time server the `track` snippet starts.
pub async fn my_server(shutdown: rt::CancelToken) {
    shutdown.cancelled().await;
}
