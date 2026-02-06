//! Advanced health check system with liveness/readiness probes.
//!
//! Provides a [`HealthIndicator`] trait for custom health checks and a builder
//! pattern for assembling multiple checks into the [`Health`](super::plugins::Health) plugin.
//!
//! # Endpoints
//!
//! | Path             | Description                                  |
//! |------------------|----------------------------------------------|
//! | `GET /health`    | Aggregated status — 200 if UP, 503 if DOWN   |
//! | `GET /health/live` | Liveness probe — always 200                |
//! | `GET /health/ready` | Readiness probe — 200 if all checks pass  |
//!
//! # Example
//!
//! ```ignore
//! use quarlus_core::health::{HealthIndicator, HealthStatus};
//!
//! struct DbHealth { pool: SqlitePool }
//!
//! impl HealthIndicator for DbHealth {
//!     fn name(&self) -> &str { "db" }
//!     async fn check(&self) -> HealthStatus {
//!         match sqlx::query("SELECT 1").fetch_one(&self.pool).await {
//!             Ok(_) => HealthStatus::Up,
//!             Err(e) => HealthStatus::Down(e.to_string()),
//!         }
//!     }
//! }
//! ```

use std::sync::Arc;

use axum::response::IntoResponse;
use serde::Serialize;

/// Result of a single health check.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HealthStatus {
    Up,
    Down(String),
}

impl HealthStatus {
    pub fn is_up(&self) -> bool {
        matches!(self, HealthStatus::Up)
    }
}

/// A named health indicator that can be registered with the health plugin.
pub trait HealthIndicator: Send + Sync + 'static {
    /// The name of this health check (e.g. `"db"`, `"redis"`).
    fn name(&self) -> &str;

    /// Perform the health check.
    fn check(&self) -> impl std::future::Future<Output = HealthStatus> + Send;
}

/// A single check result in the health response.
#[derive(Debug, Clone, Serialize)]
pub struct HealthCheck {
    pub name: String,
    pub status: HealthCheckStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HealthCheckStatus {
    Up,
    Down,
}

/// Aggregated health response.
#[derive(Debug, Clone, Serialize)]
pub struct HealthResponse {
    pub status: HealthCheckStatus,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<HealthCheck>,
}

/// Builder for assembling health checks.
pub struct HealthBuilder {
    checks: Vec<Box<dyn HealthIndicatorErased>>,
}

/// Object-safe wrapper for HealthIndicator.
pub(crate) trait HealthIndicatorErased: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn check(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = HealthStatus> + Send + '_>>;
}

impl<T: HealthIndicator> HealthIndicatorErased for T {
    fn name(&self) -> &str {
        HealthIndicator::name(self)
    }

    fn check(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = HealthStatus> + Send + '_>> {
        Box::pin(HealthIndicator::check(self))
    }
}

impl HealthBuilder {
    pub fn new() -> Self {
        Self { checks: Vec::new() }
    }

    /// Register a health check.
    pub fn check<H: HealthIndicator>(mut self, indicator: H) -> Self {
        self.checks.push(Box::new(indicator));
        self
    }

    /// Build the advanced health plugin.
    pub fn build(self) -> crate::plugins::AdvancedHealth {
        crate::plugins::AdvancedHealth::new(self.checks)
    }
}

impl Default for HealthBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared state for health check handlers.
pub(crate) struct HealthState {
    pub(crate) checks: Vec<Box<dyn HealthIndicatorErased>>,
}

impl HealthState {
    async fn aggregate(&self) -> HealthResponse {
        let mut checks = Vec::with_capacity(self.checks.len());
        let mut all_up = true;

        for indicator in &self.checks {
            let status = indicator.check().await;
            let (check_status, reason) = match &status {
                HealthStatus::Up => (HealthCheckStatus::Up, None),
                HealthStatus::Down(r) => {
                    all_up = false;
                    (HealthCheckStatus::Down, Some(r.clone()))
                }
            };
            checks.push(HealthCheck {
                name: indicator.name().to_string(),
                status: check_status,
                reason,
            });
        }

        HealthResponse {
            status: if all_up {
                HealthCheckStatus::Up
            } else {
                HealthCheckStatus::Down
            },
            checks,
        }
    }
}

/// Handler: GET /health — aggregated status.
pub(crate) async fn health_handler(
    state: axum::extract::State<Arc<HealthState>>,
) -> impl IntoResponse {
    let response = state.aggregate().await;
    let status_code = if matches!(response.status, HealthCheckStatus::Up) {
        crate::http::StatusCode::OK
    } else {
        crate::http::StatusCode::SERVICE_UNAVAILABLE
    };
    (status_code, axum::Json(response))
}

/// Handler: GET /health/live — always 200.
pub(crate) async fn liveness_handler() -> impl IntoResponse {
    (crate::http::StatusCode::OK, "OK")
}

/// Handler: GET /health/ready — 200 if all checks pass.
pub(crate) async fn readiness_handler(
    state: axum::extract::State<Arc<HealthState>>,
) -> impl IntoResponse {
    let response = state.aggregate().await;
    let status_code = if matches!(response.status, HealthCheckStatus::Up) {
        crate::http::StatusCode::OK
    } else {
        crate::http::StatusCode::SERVICE_UNAVAILABLE
    };
    (status_code, axum::Json(response))
}

