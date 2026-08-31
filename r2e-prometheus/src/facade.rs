//! `metrics`-facade backend (feature `metrics-facade`).
//!
//! For applications that are already on the [`metrics`](https://docs.rs/metrics)
//! facade with their own exporter (typically `metrics-exporter-prometheus`).
//! R2E installs **only** the HTTP tracking layer; the application keeps owning
//! the recorder, the histogram buckets and the scrape endpoint.
//!
//! ```rust,ignore
//! // main.rs — the app owns the recorder and the endpoint, as before
//! let handle = metrics_exporter_prometheus::PrometheusBuilder::new()
//!     .install_recorder()?;
//!
//! AppBuilder::new()
//!     .plugin(MetricsFacade::builder().exclude_path("/health").build())
//!     // … app's own `/metrics` route rendering `handle`
//! ```
//!
//! # Emitted metrics
//!
//! | metric | kind | labels |
//! |---|---|---|
//! | `http_requests_total` | counter | `method`, `path`, `status` |
//! | `http_request_duration_seconds` | histogram | `method`, `path` |
//! | `http_requests_in_flight` | gauge | — |
//!
//! Same names, kinds and labels as the `prometheus`-backed plugin, so a
//! dashboard written against either stack keeps working. `path` is the matched
//! route template (`/users/{id}`) or the `unmatched` sentinel, `method` is
//! bounded to the nine standard verbs plus `other` — see
//! [`crate::UNMATCHED_PATH_LABEL`] / [`crate::OTHER_METHOD_LABEL`].
//!
//! Histogram buckets are **not** configurable here: with the `metrics` facade
//! the exporter owns bucket layout (`PrometheusBuilder::set_buckets_for_metric`).

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::{OnceLock, RwLock};

use r2e_core::plugin::{PluginBuildContext, PluginBuildError};
use r2e_core::prelude::ConfigProperties;
use r2e_core::Plugin;

use crate::layer::HttpMetricsLayer;
use crate::metrics::MetricsConfig;
use crate::recorder::HttpMetricsRecorder;

/// The three metric names, resolved once (namespace prefix applied).
#[derive(Debug)]
struct MetricNames {
    requests_total: &'static str,
    duration_seconds: &'static str,
    in_flight: &'static str,
}

static UNPREFIXED: MetricNames = MetricNames {
    requests_total: "http_requests_total",
    duration_seconds: "http_request_duration_seconds",
    in_flight: "http_requests_in_flight",
};

/// Records R2E's HTTP request metrics through the `metrics` facade macros.
///
/// Cheap to clone (a `&'static` pointer). The names are resolved once at
/// construction: with a namespace they are leaked, which is bounded by the
/// number of `MetricsFacade` plugins built in the process (one, in practice).
#[derive(Clone, Copy, Debug)]
pub struct MetricsFacadeRecorder {
    names: &'static MetricNames,
}

impl Default for MetricsFacadeRecorder {
    fn default() -> Self {
        Self::new(None)
    }
}

impl MetricsFacadeRecorder {
    /// Build a recorder, optionally prefixing every metric name with
    /// `<namespace>_`.
    pub fn new(namespace: Option<&str>) -> Self {
        let names = match namespace {
            None => &UNPREFIXED,
            Some(ns) => &*Box::leak(Box::new(MetricNames {
                requests_total: intern(&format!("{ns}_http_requests_total")),
                duration_seconds: intern(&format!("{ns}_http_request_duration_seconds")),
                in_flight: intern(&format!("{ns}_http_requests_in_flight")),
            })),
        };
        Self { names }
    }

    /// Publish the metric descriptions (the `# HELP` lines of a Prometheus
    /// exporter).
    ///
    /// Best effort: the `metrics` facade routes descriptions to whatever
    /// recorder is installed *at this moment*, so install the application
    /// recorder before building the app if the descriptions matter.
    pub fn describe(&self) {
        ::metrics::describe_counter!(self.names.requests_total, "Total number of HTTP requests");
        ::metrics::describe_histogram!(
            self.names.duration_seconds,
            ::metrics::Unit::Seconds,
            "HTTP request duration in seconds"
        );
        ::metrics::describe_gauge!(
            self.names.in_flight,
            "Number of HTTP requests currently being processed"
        );
    }
}

impl HttpMetricsRecorder for MetricsFacadeRecorder {
    fn record_request(&self, method: &'static str, path: &str, status: u16, duration_secs: f64) {
        // Both label values are drawn from bounded sets (route templates +
        // sentinel, standard status codes), so interning/`&'static` reuse keeps
        // the hot path allocation-free — `metrics` label values are
        // `Cow<'static, str>`, and an owned one would mean a `String` per
        // request per metric.
        let path = intern(path);
        let status = status_label(status);
        ::metrics::counter!(
            self.names.requests_total,
            "method" => method,
            "path" => path,
            "status" => status,
        )
        .increment(1);
        ::metrics::histogram!(
            self.names.duration_seconds,
            "method" => method,
            "path" => path,
        )
        .record(duration_secs);
    }

    fn inc_in_flight(&self) {
        ::metrics::gauge!(self.names.in_flight).increment(1.0);
    }

    fn dec_in_flight(&self) {
        ::metrics::gauge!(self.names.in_flight).decrement(1.0);
    }
}

/// Intern a label value into a `&'static str`.
///
/// Only ever called with values from bounded sets (route templates, the
/// `unmatched` sentinel, metric names), so the leak is bounded by the app's
/// route table — never by request traffic. Reads take the shared lock and hit
/// on every request after the first one for a given route.
fn intern(value: &str) -> &'static str {
    static INTERNED: OnceLock<RwLock<HashSet<&'static str>>> = OnceLock::new();
    let set = INTERNED.get_or_init(|| RwLock::new(HashSet::new()));

    if let Some(found) = set.read().expect("intern lock").get(value) {
        return found;
    }
    let mut guard = set.write().expect("intern lock");
    if let Some(found) = guard.get(value) {
        return found;
    }
    let leaked: &'static str = Box::leak(value.to_owned().into_boxed_str());
    guard.insert(leaked);
    leaked
}

/// Status label without allocating for the usual codes.
fn status_label(status: u16) -> Cow<'static, str> {
    match status {
        200 => Cow::Borrowed("200"),
        201 => Cow::Borrowed("201"),
        202 => Cow::Borrowed("202"),
        204 => Cow::Borrowed("204"),
        301 => Cow::Borrowed("301"),
        302 => Cow::Borrowed("302"),
        304 => Cow::Borrowed("304"),
        400 => Cow::Borrowed("400"),
        401 => Cow::Borrowed("401"),
        403 => Cow::Borrowed("403"),
        404 => Cow::Borrowed("404"),
        405 => Cow::Borrowed("405"),
        409 => Cow::Borrowed("409"),
        422 => Cow::Borrowed("422"),
        429 => Cow::Borrowed("429"),
        500 => Cow::Borrowed("500"),
        502 => Cow::Borrowed("502"),
        503 => Cow::Borrowed("503"),
        504 => Cow::Borrowed("504"),
        other => Cow::Owned(other.to_string()),
    }
}

/// Typed configuration for the [`MetricsFacade`] plugin, read from the
/// `metrics.*` YAML section.
///
/// ```yaml
/// metrics:
///   namespace: myapp
///   exclude_paths: ["/health", "/metrics"]
/// ```
///
/// Precedence per knob: **programmatic builder setting > file config >
/// default**, like [`crate::PrometheusConfig`]. There is no `endpoint` key: the
/// application owns the scrape endpoint on this path.
#[derive(ConfigProperties, Clone, Debug, Default)]
pub struct MetricsFacadeConfig {
    /// Namespace prefix applied to every metric name.
    pub namespace: Option<String>,
    /// Request paths excluded from metrics tracking.
    pub exclude_paths: Option<Vec<String>>,
}

/// HTTP request metrics through the [`metrics`](https://docs.rs/metrics)
/// facade — the application owns the recorder and the scrape endpoint.
///
/// Installs the same tracking layer as [`crate::Prometheus`] but records
/// through [`MetricsFacadeRecorder`]. It provides no bean, mounts no route and
/// never touches this crate's `prometheus` registry, so an app on
/// `metrics` + `metrics-exporter-prometheus` keeps one metrics stack.
///
/// With `metrics.enabled = false` nothing is installed at all (the tracking
/// layer is a surface effect, dropped by the enabled gate).
///
/// ```rust,ignore
/// .plugin(MetricsFacade::new())
/// .plugin(MetricsFacade::builder().namespace("myapp").exclude_path("/health").build())
/// ```
#[derive(Default)]
pub struct MetricsFacade {
    namespace: Option<String>,
    exclude_paths: Option<Vec<String>>,
}

impl MetricsFacade {
    /// Track HTTP requests with default settings (no namespace, nothing
    /// excluded).
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder for namespace / exclusions.
    pub fn builder() -> MetricsFacadeBuilder {
        MetricsFacadeBuilder::default()
    }
}

/// Builder for [`MetricsFacade`].
#[derive(Default)]
pub struct MetricsFacadeBuilder {
    namespace: Option<String>,
    exclude_paths: Vec<String>,
}

impl MetricsFacadeBuilder {
    /// Prefix every metric name with `<namespace>_`.
    pub fn namespace(mut self, namespace: &str) -> Self {
        self.namespace = Some(namespace.to_string());
        self
    }

    /// Exclude paths from metrics tracking. See
    /// [`crate::PrometheusBuilder::exclude_paths`] for the matching semantics.
    pub fn exclude_paths(mut self, paths: &[&str]) -> Self {
        self.exclude_paths = paths.iter().map(|s| s.to_string()).collect();
        self
    }

    /// Add a single excluded path.
    pub fn exclude_path(mut self, path: &str) -> Self {
        self.exclude_paths.push(path.to_string());
        self
    }

    /// Build the plugin. Unset knobs stay `None` so file config can supply them.
    pub fn build(self) -> MetricsFacade {
        MetricsFacade {
            namespace: self.namespace,
            exclude_paths: (!self.exclude_paths.is_empty()).then_some(self.exclude_paths),
        }
    }
}

impl Plugin for MetricsFacade {
    type Provided = ();
    type Deps = ();
    type Config = MetricsFacadeConfig;
    type Controllers = ();
    const CONFIG_PREFIX: Option<&'static str> = Some("metrics");

    async fn build(
        self,
        _deps: Self::Deps,
        config: Option<Self::Config>,
        ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        // Nothing this plugin does is a process-global side effect on its own
        // (the recorder is the app's), but the gate still has to be honored
        // before the layer is registered.
        if !ctx.enabled() {
            tracing::info!(
                "MetricsFacade plugin disabled via `metrics.enabled = false`; \
                 no HTTP tracking layer is installed"
            );
            return Ok(());
        }

        let file = config.unwrap_or_default();
        let namespace = self.namespace.or(file.namespace);
        let exclude_paths = self
            .exclude_paths
            .or(file.exclude_paths)
            .unwrap_or_default();

        let recorder = MetricsFacadeRecorder::new(namespace.as_deref());
        recorder.describe();

        let layer_config = MetricsConfig {
            exclude_paths,
            ..MetricsConfig::default()
        };
        ctx.add_layer(move |router| {
            router.layer(HttpMetricsLayer::with_recorder(layer_config, recorder))
        });

        Ok(())
    }
}
