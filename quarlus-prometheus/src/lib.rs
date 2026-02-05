//! Prometheus metrics plugin for Quarlus.
//!
//! Provides automatic HTTP request tracking and a `/metrics` endpoint.
//!
//! # Usage
//!
//! ```rust,ignore
//! use quarlus_core::AppBuilder;
//! use quarlus_prometheus::Prometheus;
//!
//! AppBuilder::new()
//!     .build_state::<MyState, _>()
//!     .with(Prometheus::new("/metrics"))
//!     .serve("0.0.0.0:3000")
//!     .await;
//! ```
//!
//! # Metrics
//!
//! The following metrics are automatically tracked:
//!
//! - `http_requests_total` - Counter with labels: method, path, status
//! - `http_request_duration_seconds` - Histogram with labels: method, path
//! - `http_requests_in_flight` - Gauge showing concurrent requests

mod handler;
mod layer;
mod metrics;

pub use metrics::MetricsConfig;

use handler::metrics_handler;
use layer::PrometheusLayer;
use metrics::init_metrics;
use quarlus_core::http::routing::get;
use quarlus_core::http::Router;
use quarlus_core::Plugin;

/// Prometheus metrics plugin.
///
/// Adds:
/// - A `/metrics` endpoint (configurable path)
/// - Request tracking middleware (duration, count, status)
///
/// # Example
///
/// ```rust,ignore
/// // Simple usage
/// .with(Prometheus::new("/metrics"))
///
/// // With configuration
/// .with(Prometheus::builder()
///     .endpoint("/metrics")
///     .namespace("myapp")
///     .exclude_paths(&["/health", "/metrics"])
///     .build())
/// ```
pub struct Prometheus {
    endpoint: String,
    config: MetricsConfig,
}

impl Prometheus {
    /// Create a new Prometheus plugin with the given metrics endpoint.
    pub fn new(endpoint: &str) -> Self {
        Self {
            endpoint: endpoint.to_string(),
            config: MetricsConfig::default(),
        }
    }

    /// Create a builder for advanced configuration.
    pub fn builder() -> PrometheusBuilder {
        PrometheusBuilder::default()
    }
}

impl Plugin for Prometheus {
    fn install<T: Clone + Send + Sync + 'static>(self, app: quarlus_core::AppBuilder<T>) -> quarlus_core::AppBuilder<T> {
        // Initialize global metrics
        let config = self.config.clone();
        init_metrics(&config);

        let endpoint = self.endpoint.clone();

        // Register the /metrics endpoint
        app.register_routes(Router::new().route(&endpoint, get(metrics_handler)))
            // Add the metrics tracking layer
            .with_layer_fn(move |router| router.layer(PrometheusLayer::new(config.clone())))
    }
}

/// Builder for configuring Prometheus plugin.
#[derive(Default)]
pub struct PrometheusBuilder {
    endpoint: Option<String>,
    namespace: Option<String>,
    buckets: Option<Vec<f64>>,
    exclude_paths: Vec<String>,
}

impl PrometheusBuilder {
    /// Set the metrics endpoint path (default: "/metrics").
    pub fn endpoint(mut self, endpoint: &str) -> Self {
        self.endpoint = Some(endpoint.to_string());
        self
    }

    /// Set a namespace prefix for all metrics (e.g., "myapp" -> "myapp_http_requests_total").
    pub fn namespace(mut self, namespace: &str) -> Self {
        self.namespace = Some(namespace.to_string());
        self
    }

    /// Set custom histogram buckets for request duration.
    pub fn buckets(mut self, buckets: &[f64]) -> Self {
        self.buckets = Some(buckets.to_vec());
        self
    }

    /// Exclude paths from metrics tracking (e.g., "/health", "/metrics").
    pub fn exclude_paths(mut self, paths: &[&str]) -> Self {
        self.exclude_paths = paths.iter().map(|s| s.to_string()).collect();
        self
    }

    /// Add a single path to exclude from metrics tracking.
    pub fn exclude_path(mut self, path: &str) -> Self {
        self.exclude_paths.push(path.to_string());
        self
    }

    /// Build the Prometheus plugin.
    pub fn build(self) -> Prometheus {
        let mut config = MetricsConfig::default();

        if let Some(ns) = self.namespace {
            config.namespace = Some(ns);
        }
        if let Some(b) = self.buckets {
            config.buckets = b;
        }
        if !self.exclude_paths.is_empty() {
            config.exclude_paths = self.exclude_paths;
        }

        Prometheus {
            endpoint: self.endpoint.unwrap_or_else(|| "/metrics".to_string()),
            config,
        }
    }
}
