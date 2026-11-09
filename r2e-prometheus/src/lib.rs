//! Prometheus metrics plugin for R2E.
//!
//! Provides automatic HTTP request tracking and a `/metrics` endpoint.
//! Installs as a plugin (before `build_state()`) so the registry is a bean available for DI.
//!
//! # Usage
//!
//! ```rust,ignore
//! use r2e_core::AppBuilder;
//! use r2e_prometheus::Prometheus;
//!
//! AppBuilder::new()
//!     .plugin(Prometheus::new("/metrics"))
//!     .build_state()
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
//!     .build_state()
//!     .await
//!     .serve("0.0.0.0:3000")
//!     .await;
//! ```
//!
//! Or inject `PrometheusRegistry` into services for runtime registration.
//!
//! # Three responsibilities, three modes
//!
//! The plugin does three separable things: it (1) tracks HTTP requests through
//! a Tower layer, (2) owns the metric registry the layer records into, and
//! (3) exposes that registry at an endpoint. They are independently
//! selectable:
//!
//! | mode | layer | registry | `/metrics` |
//! |---|---|---|---|
//! | [`Prometheus::new`] / [`Prometheus::builder`] (default) | yes | this crate's `prometheus::Registry` | mounted |
//! | [`Prometheus::layer_only`] (`prometheus.expose_endpoint: false`) | yes | this crate's `prometheus::Registry` | **not** mounted — scrape it yourself with [`encode_metrics`] |
//! | `MetricsFacade` (feature `metrics-facade`) | yes | **the app's** `metrics` recorder | **not** mounted — the app owns it |
//!
//! The last row is for applications already on the
//! [`metrics`](https://docs.rs/metrics) facade with their own exporter
//! (`metrics-exporter-prometheus`, …): R2E emits the same metric names through
//! the facade macros and touches no global of its own. See the `facade` module
//! for the emitted series.
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
//! requests that no route matched (404s, `Routes::fallback`) are all recorded
//! under the single sentinel value [`UNMATCHED_PATH_LABEL`]. The `method`
//! label is capped to the nine standard HTTP methods; non-standard extension
//! methods are recorded under [`OTHER_METHOD_LABEL`].

#[cfg(feature = "metrics-facade")]
mod facade;
mod handler;
mod layer;
mod metrics;
mod recorder;
mod worker;

#[cfg(feature = "metrics-facade")]
pub use facade::{MetricsFacade, MetricsFacadeBuilder, MetricsFacadeConfig, MetricsFacadeRecorder};
pub use layer::{
    HttpMetricsLayer, HttpMetricsResponseFuture, HttpMetricsService, PrometheusLayer,
    PrometheusResponseFuture, PrometheusService, OTHER_METHOD_LABEL, UNMATCHED_PATH_LABEL,
};
pub use metrics::{encode_metrics, init_metrics, is_initialized, metrics, registry, MetricsConfig};
pub use prometheus;
pub use recorder::{HttpMetricsRecorder, PrometheusRecorder};
pub use worker::WorkerCollector;

use handler::metrics_handler;
use r2e_core::http::routing::get;
use r2e_core::config::Layered;
use r2e_core::plugin::{PluginBuildContext, PluginBuildError};
use r2e_core::prelude::ConfigProperties;
use r2e_core::Plugin;

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
///   expose_endpoint: true
///   namespace: myapp
///   buckets: [0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0]
///   exclude_paths: ["/health", "/metrics"]
/// ```
///
/// Precedence for each knob: **programmatic builder setting > this file config >
/// preset ([`Prometheus::preset`]) > default**.
#[derive(ConfigProperties, Clone, Debug, Default)]
pub struct PrometheusConfig {
    /// Metrics endpoint path (default `/metrics`).
    pub endpoint: Option<String>,
    /// Mount the metrics endpoint at all (default `true`). Set it to `false`
    /// for layer-only tracking: requests are recorded into the registry, but
    /// nothing is exported over HTTP by R2E (scrape it yourself with
    /// [`encode_metrics`], or push it).
    pub expose_endpoint: Option<bool>,
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
    /// Never panics: if the plugin's `configure` step has not initialized the
    /// registry (notably when the plugin is disabled via
    /// `prometheus.enabled = false`), a default-configured registry is lazily
    /// created — real and registrable, just not exported at the metrics
    /// endpoint.
    pub fn inner(&self) -> &'static prometheus::Registry {
        registry()
    }
}

/// Prometheus metrics plugin.
///
/// Installs as a [`Plugin`], providing a [`PrometheusRegistry`] bean
/// for dependency injection. The `/metrics` endpoint and tracking middleware
/// are installed via a deferred action after state resolution.
///
/// # `prometheus.enabled = false`
///
/// `build` returns immediately with the (still required) `PrometheusRegistry`
/// bean and does **not** call `init_metrics`: the process-global metrics
/// registry stays untouched and no collector — including the builder's
/// `register(..)` list — is installed. The effect gate alone would not have
/// covered this, since `init_metrics` runs inside `build`, not in an effect.
/// The bean remains usable (its accessors lazily default-initialize the global
/// on first *use*), so injecting it anywhere keeps compiling and running.
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
    // builder setter > `prometheus.*` > preset > default, merged in `build`.
    cfg: Layered<PrometheusConfig>,
    collectors: Vec<Box<dyn prometheus::core::Collector>>,
}

impl Prometheus {
    /// Create a new Prometheus plugin with the given metrics endpoint.
    ///
    /// The endpoint is treated as an explicit builder setting, so it takes
    /// precedence over a `prometheus.endpoint` file value. To let file config
    /// drive the endpoint, use [`Prometheus::builder`] without `.endpoint(..)`.
    pub fn new(endpoint: &str) -> Self {
        PrometheusBuilder::default().endpoint(endpoint).build()
    }

    /// A Prometheus plugin whose defaults come from `config` — a shared
    /// baseline that sits **below** the `prometheus.*` section: the app's
    /// file overrides it key by key. Use
    /// [`PrometheusBuilder::preset`] to combine it with builder settings.
    pub fn preset(config: PrometheusConfig) -> Self {
        PrometheusBuilder::default().preset(config).build()
    }

    /// HTTP tracking **without** the `/metrics` endpoint.
    ///
    /// The registry is still initialized (with the merged builder/file config)
    /// and requests are still recorded into it — R2E just does not serve it.
    /// Use it when the exposition is the application's business: an endpoint of
    /// its own rendering [`encode_metrics`], a push gateway, or a sidecar.
    ///
    /// Shorthand for `Prometheus::builder().without_endpoint().build()`; the
    /// same switch is reachable from config as `prometheus.expose_endpoint:
    /// false`.
    ///
    /// If the application is on the [`metrics`](https://docs.rs/metrics) facade
    /// rather than the `prometheus` crate, use `MetricsFacade` (feature
    /// `metrics-facade`) instead — same layer, the app's recorder.
    pub fn layer_only() -> Self {
        PrometheusBuilder::default().without_endpoint().build()
    }

    /// Create a builder for advanced configuration.
    pub fn builder() -> PrometheusBuilder {
        PrometheusBuilder::default()
    }
}

/// Turn the merged [`PrometheusConfig`] ([`Layered::resolve`]: builder > file
/// > preset) into the effective endpoint, whether it is mounted, and the
/// [`MetricsConfig`], applying the built-in defaults.
///
/// Exposed (hidden) so the defaults can be unit-tested without the global
/// metrics singleton.
#[doc(hidden)]
pub fn resolve_config(merged: PrometheusConfig) -> (String, bool, MetricsConfig) {
    let endpoint = merged.endpoint.unwrap_or_else(|| "/metrics".to_string());
    let expose_endpoint = merged.expose_endpoint.unwrap_or(true);

    let mut config = MetricsConfig::default();
    config.namespace = merged.namespace;
    if let Some(buckets) = merged.buckets {
        config.buckets = buckets;
    }
    if let Some(exclude_paths) = merged.exclude_paths {
        config.exclude_paths = exclude_paths;
    }
    (endpoint, expose_endpoint, config)
}

impl Plugin for Prometheus {
    type Provided = (PrometheusRegistry,);
    type Deps = ();
    type Config = PrometheusConfig;
    type Controllers = ();
    const CONFIG_PREFIX: Option<&'static str> = Some("prometheus");

    async fn build(
        self,
        _deps: Self::Deps,
        config: Option<Self::Config>,
        ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        let Prometheus { cfg, collectors } = self;
        let (endpoint, expose_endpoint, metrics_config) = resolve_config(cfg.resolve(config));

        // `build` runs even when the plugin is disabled (the `PrometheusRegistry`
        // bean is in the compile-time provision list), but its *side effects*
        // must not: initializing the global `METRICS` singleton is a
        // process-wide, one-shot write, and registering custom collectors into
        // it is permanent. With `prometheus.enabled = false` the global is left
        // untouched — `metrics()` lazily default-initializes for anyone who
        // injects the bean, and nothing is exported (the endpoint and tracking
        // layer are enabled-gated effects anyway).
        if !ctx.enabled() {
            tracing::info!(
                "Prometheus plugin disabled via `prometheus.enabled = false`; \
                 the global metrics registry is left untouched and no collector is registered"
            );
            return Ok((PrometheusRegistry,));
        }

        // Initialize the global metrics singleton with the merged config.
        if is_initialized() {
            // Something (e.g. a provided-bean post_construct running earlier in
            // the graph) touched the lazily-default-initialized registry first —
            // the merged config below is then a no-op for metric setup.
            tracing::warn!(
                "Prometheus metrics were initialized before the plugin's build step; \
                 builder/file configuration (namespace, buckets) may not have been applied"
            );
        }
        let m = init_metrics(&metrics_config);
        for collector in collectors {
            m.registry
                .register(collector)
                .map_err(|e| format!("Failed to register custom Prometheus collector: {e}"))?;
        }

        // The /metrics route (unless this is a layer-only install) is mounted
        // as a Routes-stage effect, so it lands INSIDE the layer stack like a
        // controller route — below the catch-panic slot and every layer
        // installed before this plugin. Mounting it from the `add_layer`
        // closure instead would place it above all of those (see
        // `docs/claude/plugins.md`, "`add_layer` wraps, it never mounts").
        if expose_endpoint {
            ctx.after_routes(move |routes| {
                routes.register_routes(
                    r2e_core::http::Routes::new().route(&endpoint, get(|_, _| metrics_handler())),
                );
            });
        }
        ctx.add_layer_in(r2e_core::LayerSlot::Metrics, move |router| {
            router.layer(PrometheusLayer::new(metrics_config))
        });

        Ok((PrometheusRegistry,))
    }
}

fn unset_empty_exclusions(config: &mut PrometheusConfig) {
    if config.exclude_paths.as_ref().is_some_and(Vec::is_empty) {
        config.exclude_paths = None;
    }
}

/// Builder for configuring Prometheus plugin.
#[derive(Default)]
pub struct PrometheusBuilder {
    cfg: Layered<PrometheusConfig>,
    collectors: Vec<Box<dyn prometheus::core::Collector>>,
}

impl PrometheusBuilder {
    /// Defaults that sit **below** the `prometheus.*` section (and below
    /// every setter of this builder) — see [`Prometheus::preset`].
    pub fn preset(mut self, config: PrometheusConfig) -> Self {
        self.cfg.replace_preset(config);
        self
    }

    /// Set the metrics endpoint path (default: "/metrics").
    pub fn endpoint(mut self, endpoint: &str) -> Self {
        self.cfg.set(|c| c.endpoint = Some(endpoint.to_string()));
        self
    }

    /// Do not mount the metrics endpoint: track requests, export nothing.
    ///
    /// Overrides `prometheus.expose_endpoint` from file config, like every
    /// other builder setting. See [`Prometheus::layer_only`].
    pub fn without_endpoint(mut self) -> Self {
        self.cfg.set(|c| c.expose_endpoint = Some(false));
        self
    }

    /// Set a namespace prefix for all metrics (e.g., "myapp" -> "myapp_http_requests_total").
    pub fn namespace(mut self, namespace: &str) -> Self {
        self.cfg.set(|c| c.namespace = Some(namespace.to_string()));
        self
    }

    /// Set custom histogram buckets for request duration.
    pub fn buckets(mut self, buckets: &[f64]) -> Self {
        self.cfg.set(|c| c.buckets = Some(buckets.to_vec()));
        self
    }

    /// Exclude paths from metrics tracking (e.g., "/health", "/metrics").
    ///
    /// Entries are prefix-matched against both the raw request path
    /// (`/users/5`) and the route-template label the request would be
    /// recorded under (`/users/{id}`), so either spelling works.
    pub fn exclude_paths(mut self, paths: &[&str]) -> Self {
        self.cfg
            .set(|c| c.exclude_paths = Some(paths.iter().map(|s| s.to_string()).collect()));
        self
    }

    /// Add a single path to exclude from metrics tracking.
    ///
    /// See [`Self::exclude_paths`] for the matching semantics.
    pub fn exclude_path(mut self, path: &str) -> Self {
        self.cfg.set(|c| {
            c.exclude_paths
                .get_or_insert_with(Vec::new)
                .push(path.to_string());
        });
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
    pub fn build(mut self) -> Prometheus {
        // An empty exclusion list is "not set", as it always was: the file
        // (then the preset) still supplies one.
        self.cfg.set(unset_empty_exclusions);
        Prometheus {
            cfg: self.cfg,
            collectors: self.collectors,
        }
    }
}
