//! Prometheus metrics plugin for R2E.
//!
//! Provides automatic HTTP request tracking and a `/metrics` endpoint.
//! Installs as a pre-state plugin so the registry is available for DI.
//!
//! # Usage
//!
//! ```rust,ignore
//! use r2e_core::AppBuilder;
//! use r2e_prometheus::Prometheus;
//!
//! AppBuilder::new()
//!     .plugin(Prometheus::new("/metrics"))
//!     .build_state::<MyState, _, _>()
//!     .await
//!     .serve("0.0.0.0:3000")
//!     .await;
//! ```
//!
//! # Custom Metrics
//!
//! Register custom collectors at build time:
//!
//! ```rust,ignore
//! use r2e_prometheus::prometheus::IntCounter;
//!
//! let my_counter = IntCounter::new("my_counter", "A custom counter").unwrap();
//! AppBuilder::new()
//!     .plugin(Prometheus::builder()
//!         .namespace("myapp")
//!         .register(Box::new(my_counter.clone()))
//!         .build())
//!     .build_state::<MyState, _, _>()
//!     .await
//!     .serve("0.0.0.0:3000")
//!     .await;
//! ```
//!
//! Or inject `PrometheusRegistry` into services for runtime registration.
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

pub use metrics::{encode_metrics, init_metrics, is_initialized, metrics, registry, MetricsConfig};
pub use prometheus;

use handler::metrics_handler;
use layer::PrometheusLayer;
use r2e_core::http::routing::get;
use r2e_core::{DeferredAction, PluginInstallContext, PreStatePlugin};

/// A handle to the shared Prometheus metrics registry.
///
/// Clone is cheap (`prometheus::Registry` uses `Arc` internally).
/// Inject this into services to register custom metrics at runtime.
///
/// # Example
///
/// ```rust,ignore
/// #[derive(Bean)]
/// pub struct MyService {
///     #[inject] registry: PrometheusRegistry,
/// }
///
/// impl MyService {
///     fn setup(&self) {
///         let counter = prometheus::IntCounter::new("my_counter", "help").unwrap();
///         self.registry.register(Box::new(counter)).unwrap();
///     }
/// }
/// ```
#[derive(Clone)]
pub struct PrometheusRegistry {
    inner: prometheus::Registry,
}

impl PrometheusRegistry {
    /// Register a custom Prometheus collector (counter, gauge, histogram, etc.).
    pub fn register(
        &self,
        collector: Box<dyn prometheus::core::Collector>,
    ) -> prometheus::Result<()> {
        self.inner.register(collector)
    }

    /// Access the underlying `prometheus::Registry`.
    pub fn inner(&self) -> &prometheus::Registry {
        &self.inner
    }
}

/// Prometheus metrics plugin.
///
/// Installs as a [`PreStatePlugin`], providing a [`PrometheusRegistry`] bean
/// for dependency injection. The `/metrics` endpoint and tracking middleware
/// are installed via a deferred action after state resolution.
///
/// # Example
///
/// ```rust,ignore
/// // Simple usage
/// .plugin(Prometheus::new("/metrics"))
///
/// // With configuration
/// .plugin(Prometheus::builder()
///     .endpoint("/metrics")
///     .namespace("myapp")
///     .exclude_path("/health")
///     .exclude_path("/metrics")
///     .register(Box::new(my_custom_counter))
///     .build())
/// ```
pub struct Prometheus {
    endpoint: String,
    config: MetricsConfig,
    collectors: Vec<Box<dyn prometheus::core::Collector>>,
}

impl Prometheus {
    /// Create a new Prometheus plugin with the given metrics endpoint.
    pub fn new(endpoint: &str) -> Self {
        Self {
            endpoint: endpoint.to_string(),
            config: MetricsConfig::default(),
            collectors: Vec::new(),
        }
    }

    /// Create a builder for advanced configuration.
    pub fn builder() -> PrometheusBuilder {
        PrometheusBuilder::default()
    }
}

impl PreStatePlugin for Prometheus {
    type Provided = PrometheusRegistry;
    type Required = r2e_core::type_list::TNil;

    fn install(self, ctx: &mut PluginInstallContext) -> Self::Provided {
        // Initialize global metrics singleton
        let m = init_metrics(&self.config);

        // Register user-supplied collectors
        for collector in self.collectors {
            m.registry
                .register(collector)
                .expect("Failed to register custom Prometheus collector");
        }

        // Create the injectable handle (cheap Arc clone)
        let handle = PrometheusRegistry {
            inner: m.registry.clone(),
        };

        // Defer layer + route installation to post-state phase
        let config = self.config;
        let endpoint = self.endpoint;
        ctx.add_deferred(DeferredAction::new("Prometheus", move |dctx| {
            dctx.add_layer(Box::new(move |router| {
                router
                    .route(&endpoint, get(metrics_handler))
                    .layer(PrometheusLayer::new(config))
            }));
        }));

        handle
    }
}

/// Builder for configuring Prometheus plugin.
#[derive(Default)]
pub struct PrometheusBuilder {
    endpoint: Option<String>,
    namespace: Option<String>,
    buckets: Option<Vec<f64>>,
    exclude_paths: Vec<String>,
    collectors: Vec<Box<dyn prometheus::core::Collector>>,
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

    /// Register a custom Prometheus collector that will be added to the shared registry.
    ///
    /// Collectors are registered during plugin installation, so they appear
    /// alongside built-in HTTP metrics on the `/metrics` endpoint.
    pub fn register(mut self, collector: Box<dyn prometheus::core::Collector>) -> Self {
        self.collectors.push(collector);
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
            collectors: self.collectors,
        }
    }
}
