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
//! use r2e_core::health::{HealthIndicator, HealthStatus};
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
use std::time::{Duration, Instant};

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

    /// Whether this check affects the readiness probe (default: `true`).
    ///
    /// Liveness-only checks (e.g. disk space) return `false` so they don't
    /// block readiness.
    fn affects_readiness(&self) -> bool {
        true
    }
}

/// A single check result in the health response.
#[derive(Debug, Clone, Serialize)]
pub struct HealthCheck {
    pub name: String,
    pub status: HealthCheckStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime_seconds: Option<u64>,
}

/// Builder for assembling health checks.
pub struct HealthBuilder {
    checks: Vec<Box<dyn HealthIndicatorErased>>,
    cache_ttl: Option<Duration>,
}

/// Object-safe wrapper for HealthIndicator.
#[doc(hidden)]
pub trait HealthIndicatorErased: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn check(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = HealthStatus> + Send + '_>>;
    fn affects_readiness(&self) -> bool;
}

impl<T: HealthIndicator> HealthIndicatorErased for T {
    fn name(&self) -> &str {
        HealthIndicator::name(self)
    }

    fn check(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = HealthStatus> + Send + '_>> {
        Box::pin(HealthIndicator::check(self))
    }

    fn affects_readiness(&self) -> bool {
        HealthIndicator::affects_readiness(self)
    }
}

impl HealthBuilder {
    pub fn new() -> Self {
        Self {
            checks: Vec::new(),
            cache_ttl: None,
        }
    }

    /// Register a health check.
    pub fn check<H: HealthIndicator>(mut self, indicator: H) -> Self {
        self.checks.push(Box::new(indicator));
        self
    }

    /// Set cache TTL for health check results.
    ///
    /// When set, health check results are cached and re-used for the
    /// specified duration before re-running the checks.
    pub fn cache_ttl(mut self, ttl: Duration) -> Self {
        self.cache_ttl = Some(ttl);
        self
    }

    /// Build the advanced health plugin.
    pub fn build(self) -> crate::plugins::AdvancedHealth {
        crate::plugins::AdvancedHealth::new(self.checks, self.cache_ttl)
    }
}

impl Default for HealthBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared state for health check handlers.
#[doc(hidden)]
pub struct HealthState {
    #[doc(hidden)]
    pub checks: Vec<Box<dyn HealthIndicatorErased>>,
    #[doc(hidden)]
    pub start_time: Instant,
    #[doc(hidden)]
    pub cache_ttl: Option<Duration>,
    #[doc(hidden)]
    pub cache: tokio::sync::RwLock<Option<(HealthResponse, Instant)>>,
}

impl HealthState {
    #[doc(hidden)]
    pub async fn aggregate(&self) -> HealthResponse {
        // Check cache
        if let Some(ttl) = self.cache_ttl {
            let cache = self.cache.read().await;
            if let Some((ref response, ref timestamp)) = *cache {
                if timestamp.elapsed() < ttl {
                    return response.clone();
                }
            }
        }

        let mut checks = Vec::with_capacity(self.checks.len());
        let mut all_up = true;

        for indicator in &self.checks {
            let start = Instant::now();
            let status = indicator.check().await;
            let duration_ms = start.elapsed().as_millis() as u64;

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
                duration_ms: Some(duration_ms),
                details: None,
            });
        }

        let response = HealthResponse {
            status: if all_up {
                HealthCheckStatus::Up
            } else {
                HealthCheckStatus::Down
            },
            checks,
            uptime_seconds: Some(self.start_time.elapsed().as_secs()),
        };

        // Update cache
        if self.cache_ttl.is_some() {
            let mut cache = self.cache.write().await;
            *cache = Some((response.clone(), Instant::now()));
        }

        response
    }

    /// Aggregate only checks that affect readiness.
    #[doc(hidden)]
    pub async fn aggregate_readiness(&self) -> HealthResponse {
        // Check cache — readiness uses the same cache as full health
        if let Some(ttl) = self.cache_ttl {
            let cache = self.cache.read().await;
            if let Some((ref response, ref timestamp)) = *cache {
                if timestamp.elapsed() < ttl {
                    // Filter to only readiness-affecting checks
                    let readiness_checks: Vec<_> = response
                        .checks
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| {
                            self.checks
                                .get(*i)
                                .map(|c| c.affects_readiness())
                                .unwrap_or(true)
                        })
                        .map(|(_, c)| c.clone())
                        .collect();
                    let all_up = readiness_checks
                        .iter()
                        .all(|c| matches!(c.status, HealthCheckStatus::Up));
                    return HealthResponse {
                        status: if all_up {
                            HealthCheckStatus::Up
                        } else {
                            HealthCheckStatus::Down
                        },
                        checks: readiness_checks,
                        uptime_seconds: Some(self.start_time.elapsed().as_secs()),
                    };
                }
            }
        }

        let mut checks = Vec::new();
        let mut all_up = true;

        for indicator in &self.checks {
            if !indicator.affects_readiness() {
                continue;
            }
            let start = Instant::now();
            let status = indicator.check().await;
            let duration_ms = start.elapsed().as_millis() as u64;

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
                duration_ms: Some(duration_ms),
                details: None,
            });
        }

        HealthResponse {
            status: if all_up {
                HealthCheckStatus::Up
            } else {
                HealthCheckStatus::Down
            },
            checks,
            uptime_seconds: Some(self.start_time.elapsed().as_secs()),
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

/// Handler: GET /health/ready — 200 if all readiness-affecting checks pass.
pub(crate) async fn readiness_handler(
    state: axum::extract::State<Arc<HealthState>>,
) -> impl IntoResponse {
    let response = state.aggregate_readiness().await;
    let status_code = if matches!(response.status, HealthCheckStatus::Up) {
        crate::http::StatusCode::OK
    } else {
        crate::http::StatusCode::SERVICE_UNAVAILABLE
    };
    (status_code, axum::Json(response))
}
