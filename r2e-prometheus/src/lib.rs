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
//!     .build_state::<MyState, _>()
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
//!     .build_state::<MyState, _>()
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
//!
//! Label cardinality is bounded: the `path` label is the matched route
//! template (e.g. `/users/{id}`), capped by the number of registered routes;
//! requests that no route matched (404s, `Router::fallback`) are all recorded
//! under the single sentinel value [`UNMATCHED_PATH_LABEL`]. The `method`
//! label is capped to the nine standard HTTP methods; non-standard extension
//! methods are recorded under [`OTHER_METHOD_LABEL`].

mod handler;
mod layer;
mod metrics;

pub use layer::{PrometheusLayer, OTHER_METHOD_LABEL, UNMATCHED_PATH_LABEL};
pub use metrics::{encode_metrics, init_metrics, is_initialized, metrics, registry, MetricsConfig};
pub use prometheus;

use handler::metrics_handler;
use r2e_core::http::routing::get;
use r2e_core::prelude::ConfigProperties;
use r2e_core::{DeferredContext, PluginInstallContext, PreStatePlugin};

/// Typed configuration for the [`Prometheus`] plugin, read from the
/// `prometheus.*` YAML section.
///
/// Every field is optional: an absent key falls through to the programmatic
/// builder setting (which takes precedence) or the built-in default. The
/// section as a whole is optional too — omit it entirely and the plugin runs on
/// builder settings / defaults.
///
/// ```yaml
/// prometheus:
///   endpoint: /metrics
///   namespace: myapp
///   buckets: [0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0]
///   exclude_paths: ["/health", "/metrics"]
/// ```
///
/// Precedence for each knob: **programmatic builder setting > this file config >
/// default**.
#[derive(ConfigProperties, Clone, Debug, Default)]
pub struct PrometheusConfig {
    /// Metrics endpoint path (default `/metrics`).
    pub endpoint: Option<String>,
    /// Namespace prefix applied to every metric name.
    pub namespace: Option<String>,
    /// Histogram buckets for request duration, in seconds.
    pub buckets: Option<Vec<f64>>,
    /// Request paths excluded from metrics tracking.
    pub exclude_paths: Option<Vec<String>>,
}

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
#[derive(Clone, Default)]
pub struct PrometheusRegistry;

impl PrometheusRegistry {
    /// Register a custom Prometheus collector (counter, gauge, histogram, etc.)
    /// on the shared global registry.
    ///
    /// The global registry is initialized by the plugin's `configure` step
    /// (during `build_state()`), so this is safe to call from runtime code —
    /// request handlers, serve hooks, or a service method invoked after startup.
    pub fn register(
        &self,
        collector: Box<dyn prometheus::core::Collector>,
    ) -> prometheus::Result<()> {
        registry().register(collector)
    }

    /// Access the shared global `prometheus::Registry`.
    ///
    /// # Panics
    ///
    /// Panics if metrics have not been initialized yet (i.e. before the
    /// plugin's `configure` step has run during `build_state()`).
    pub fn inner(&self) -> &'static prometheus::Registry {
        registry()
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
    // Programmatic overrides. `None` means "not set by the builder" — the value
    // falls through to file config, then the default. This lets the builder
    // take precedence over file config only where it was explicitly set.
    endpoint: Option<String>,
    namespace: Option<String>,
    buckets: Option<Vec<f64>>,
    exclude_paths: Option<Vec<String>>,
    collectors: Vec<Box<dyn prometheus::core::Collector>>,
}

impl Prometheus {
    /// Create a new Prometheus plugin with the given metrics endpoint.
    ///
    /// The endpoint is treated as an explicit builder setting, so it takes
    /// precedence over a `prometheus.endpoint` file value. To let file config
    /// drive the endpoint, use [`Prometheus::builder`] without `.endpoint(..)`.
    pub fn new(endpoint: &str) -> Self {
        Self {
            endpoint: Some(endpoint.to_string()),
            namespace: None,
            buckets: None,
            exclude_paths: None,
            collectors: Vec::new(),
        }
    }

    /// Create a builder for advanced configuration.
    pub fn builder() -> PrometheusBuilder {
        PrometheusBuilder::default()
    }
}

/// Merge programmatic builder settings (highest precedence), the loaded file
/// [`PrometheusConfig`] section, and built-in defaults into the effective
/// endpoint + [`MetricsConfig`].
///
/// Exposed (hidden) so the precedence contract can be unit-tested without the
/// global metrics singleton.
#[doc(hidden)]
pub fn resolve_config(
    endpoint: Option<String>,
    namespace: Option<String>,
    buckets: Option<Vec<f64>>,
    exclude_paths: Option<Vec<String>>,
    file: Option<PrometheusConfig>,
) -> (String, MetricsConfig) {
    let file = file.unwrap_or_default();
    let endpoint = endpoint
        .or(file.endpoint)
        .unwrap_or_else(|| "/metrics".to_string());

    let mut config = MetricsConfig::default();
    if let Some(ns) = namespace.or(file.namespace) {
        config.namespace = Some(ns);
    }
    if let Some(b) = buckets.or(file.buckets) {
        config.buckets = b;
    }
    if let Some(ep) = exclude_paths.or(file.exclude_paths) {
        config.exclude_paths = ep;
    }
    (endpoint, config)
}

impl PreStatePlugin for Prometheus {
    type Provided = (PrometheusRegistry,);
    type Deps = ();
    type LateDeps = ();
    type Config = PrometheusConfig;
    const CONFIG_PREFIX: Option<&'static str> = Some("prometheus");

    fn install(&mut self, (): (), _ctx: &mut PluginInstallContext<'_>) -> Self::Provided {
        // All config-dependent work (metric init, custom collectors, the layer
        // and route) is deferred to `configure`, where file config is
        // guaranteed loaded. The injectable handle delegates to the global
        // registry that `configure` initializes.
        (PrometheusRegistry,)
    }

    fn configure(
        self,
        _provided: &Self::Provided,
        (): (),
        config: Option<PrometheusConfig>,
        ctx: &mut DeferredContext<'_>,
    ) {
        let Prometheus {
            endpoint,
            namespace,
            buckets,
            exclude_paths,
            collectors,
        } = self;
        let (endpoint, metrics_config) =
            resolve_config(endpoint, namespace, buckets, exclude_paths, config);

        // Initialize the global metrics singleton with the merged config.
        let m = init_metrics(&metrics_config);
        for collector in collectors {
            m.registry
                .register(collector)
                .expect("Failed to register custom Prometheus collector");
        }

        // Install the /metrics route + tracking layer post-state.
        ctx.add_layer(Box::new(move |router| {
            router
                .route(&endpoint, get(metrics_handler))
                .layer(PrometheusLayer::new(metrics_config))
        }));
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
    ///
    /// Entries are prefix-matched against both the raw request path
    /// (`/users/5`) and the route-template label the request would be
    /// recorded under (`/users/{id}`), so either spelling works.
    pub fn exclude_paths(mut self, paths: &[&str]) -> Self {
        self.exclude_paths = paths.iter().map(|s| s.to_string()).collect();
        self
    }

    /// Add a single path to exclude from metrics tracking.
    ///
    /// See [`Self::exclude_paths`] for the matching semantics.
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

    /// Register multiple Prometheus collectors at once.
    ///
    /// Accepts any iterator yielding boxed collectors, enabling inline usage:
    ///
    /// ```rust,ignore
    /// .plugin(Prometheus::builder()
    ///     .namespace("myapp")
    ///     .register_all(my_collectors())
    ///     .build())
    /// ```
    pub fn register_all(
        mut self,
        collectors: impl IntoIterator<Item = Box<dyn prometheus::core::Collector>>,
    ) -> Self {
        self.collectors.extend(collectors);
        self
    }

    /// Build the Prometheus plugin.
    ///
    /// Builder settings left unset stay `None` so file config (then defaults)
    /// can supply them; settings that were called take precedence over file
    /// config.
    pub fn build(self) -> Prometheus {
        Prometheus {
            endpoint: self.endpoint,
            namespace: self.namespace,
            buckets: self.buckets,
            exclude_paths: (!self.exclude_paths.is_empty()).then_some(self.exclude_paths),
            collectors: self.collectors,
        }
    }
}
