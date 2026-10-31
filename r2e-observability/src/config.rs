/// Configuration for the observability stack.
///
/// The console subscriber is **not** configured here: it is the app's one
/// framework subscriber, configured by the top-level `tracing:` section (see
/// `docs/migration/observability-tracing.md`). This struct only drives the
/// OpenTelemetry side — exporter, sampler, resource, propagation.
///
/// `#[non_exhaustive]`: build it with [`new`](Self::new),
/// [`from_env`](Self::from_env) or [`from_r2e_config`](Self::from_r2e_config)
/// and the `with_*` methods.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ObservabilityConfig {
    /// Service name reported to the tracing backend.
    pub service_name: String,
    /// Service version (used in resource attributes).
    pub service_version: Option<String>,
    /// OTLP/HTTP traces endpoint (default: "http://localhost:4318/v1/traces").
    pub otlp_endpoint: String,
    /// Protocol requested by configuration. Only OTLP/HTTP is currently exported.
    pub otlp_protocol: OtlpProtocol,
    /// Whether to enable tracing export.
    pub tracing_enabled: bool,
    /// Sampling ratio (0.0 to 1.0, default 1.0 = all traces).
    pub sampling_ratio: f64,
    /// Propagation format: W3c (default), B3, or Jaeger.
    pub propagation_format: PropagationFormat,
    /// Additional resource attributes (key, value).
    pub resource_attributes: Vec<(String, String)>,
    /// Headers to forward as span attributes.
    pub capture_headers: Vec<String>,
    /// `observability.tracing.*` keys other than `enabled` found by
    /// [`from_r2e_config`](Self::from_r2e_config): the removed subscriber
    /// knobs. Reported once by the plugin's `setup`.
    pub(crate) leftover_tracing_keys: Vec<String>,
}

/// The former `observability.tracing.*` subscriber knobs, now read from the
/// top-level `tracing:` section only.
mod tracing_removed {
    pub trait Sealed {}
}

/// Marker for the **removed** `ObservabilityConfig` subscriber knobs.
///
/// Nothing implements it (its supertrait is sealed and unimplemented), so the
/// only thing it can do is turn a leftover
/// [`with_tracing_config`](ObservabilityConfig::with_tracing_config) /
/// [`with_log_format`](ObservabilityConfig::with_log_format) call into a
/// compile error that names the migration. See
/// `docs/migration/observability-tracing.md`.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "`ObservabilityConfig::with_tracing_config`/`with_log_format` were removed: the subscriber is configured by the app's top-level `tracing:` section (see docs/migration/observability-tracing.md)",
    label = "the Observability plugin no longer builds a subscriber",
    note = "delete the call and move the settings to the top-level `tracing:` section of application.yaml (`tracing.format: json`, `tracing.filter: ...`)",
    note = "the plugin now contributes its OpenTelemetry layer and the `trace_id`/`span_id` log fields to R2E's one subscriber"
)]
pub trait ObservabilityTracingRemoved: tracing_removed::Sealed {}

/// OTLP transport protocol.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OtlpProtocol {
    Grpc,
    #[default]
    Http,
}

/// Trace context propagation format.
#[derive(Debug, Clone, Default)]
pub enum PropagationFormat {
    #[default]
    W3c,
    B3,
    Jaeger,
}

impl ObservabilityConfig {
    /// Create a new config with the given service name and sensible defaults.
    pub fn new(service_name: &str) -> Self {
        Self {
            service_name: service_name.to_string(),
            service_version: None,
            otlp_endpoint: "http://localhost:4318/v1/traces".to_string(),
            otlp_protocol: OtlpProtocol::Http,
            tracing_enabled: true,
            sampling_ratio: 1.0,
            propagation_format: PropagationFormat::W3c,
            resource_attributes: Vec::new(),
            capture_headers: Vec::new(),
            leftover_tracing_keys: Vec::new(),
        }
    }

    pub fn with_service_version(mut self, version: &str) -> Self {
        self.service_version = Some(version.to_string());
        self
    }

    pub fn with_endpoint(mut self, endpoint: &str) -> Self {
        self.otlp_endpoint = endpoint.to_string();
        self
    }

    pub fn with_protocol(mut self, protocol: OtlpProtocol) -> Self {
        self.otlp_protocol = protocol;
        self
    }

    pub fn with_sampling_ratio(mut self, ratio: f64) -> Self {
        self.sampling_ratio = ratio.clamp(0.0, 1.0);
        self
    }

    pub fn with_propagation(mut self, format: PropagationFormat) -> Self {
        self.propagation_format = format;
        self
    }

    pub fn with_resource_attribute(mut self, key: &str, value: &str) -> Self {
        self.resource_attributes
            .push((key.to_string(), value.to_string()));
        self
    }

    pub fn capture_header(mut self, header: &str) -> Self {
        self.capture_headers.push(header.to_string());
        self
    }

    /// **Removed.** The plugin no longer builds a subscriber: the console
    /// output is configured by the app's top-level `tracing:` section.
    ///
    /// This shim exists only so the leftover call fails at **compile time**
    /// with a message that names the migration. Its bound,
    /// [`ObservabilityTracingRemoved`], is implemented by nothing. See
    /// `docs/migration/observability-tracing.md`.
    #[doc(hidden)]
    #[deprecated(
        note = "`ObservabilityConfig::with_tracing_config` was removed: configure the subscriber with the app's top-level `tracing:` section — see docs/migration/observability-tracing.md"
    )]
    pub fn with_tracing_config<T: ObservabilityTracingRemoved>(self, _config: T) -> Self {
        self
    }

    /// **Removed.** See [`with_tracing_config`](Self::with_tracing_config):
    /// set `tracing.format` in the app's top-level `tracing:` section instead.
    #[doc(hidden)]
    #[deprecated(
        note = "`ObservabilityConfig::with_log_format` was removed: set `tracing.format` in the app's top-level `tracing:` section — see docs/migration/observability-tracing.md"
    )]
    pub fn with_log_format<T: ObservabilityTracingRemoved>(self, _format: T) -> Self {
        self
    }

    pub fn disable_tracing(mut self) -> Self {
        self.tracing_enabled = false;
        self
    }

    /// Apply the standard OpenTelemetry environment variables supported by R2E.
    ///
    /// `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` takes precedence over
    /// `OTEL_EXPORTER_OTLP_ENDPOINT`, and `OTEL_SERVICE_NAME` overrides the
    /// supplied default. Ratio-based samplers read `OTEL_TRACES_SAMPLER_ARG`.
    pub fn from_env(service_name: &str) -> Self {
        let mut cfg = Self::new(service_name);

        if let Some(name) = non_empty_env("OTEL_SERVICE_NAME") {
            cfg.service_name = name;
        }
        if let Some(endpoint) = otlp_endpoint_from_env() {
            cfg.otlp_endpoint = endpoint;
        }
        if let Some(protocol) = non_empty_env("OTEL_EXPORTER_OTLP_TRACES_PROTOCOL")
            .or_else(|| non_empty_env("OTEL_EXPORTER_OTLP_PROTOCOL"))
            .and_then(|value| parse_otlp_protocol(&value))
        {
            cfg.otlp_protocol = protocol;
        }

        match non_empty_env("OTEL_TRACES_SAMPLER").as_deref() {
            Some("always_off") | Some("parentbased_always_off") => cfg.sampling_ratio = 0.0,
            Some("always_on") | Some("parentbased_always_on") => cfg.sampling_ratio = 1.0,
            Some("traceidratio") | Some("parentbased_traceidratio") | None => {
                if let Some(ratio) = non_empty_env("OTEL_TRACES_SAMPLER_ARG")
                    .and_then(|value| value.parse::<f64>().ok())
                {
                    cfg.sampling_ratio = ratio.clamp(0.0, 1.0);
                }
            }
            Some(_) => {}
        }

        cfg
    }

    /// Load from R2eConfig with prefix `observability`.
    ///
    /// Reads keys like:
    /// - `observability.otlp-endpoint`
    /// - `observability.otlp-protocol`
    /// - `observability.sampling-ratio`
    /// - `observability.tracing.enabled` (the OTLP export switch)
    ///
    /// The console subscriber is configured by the top-level `tracing:`
    /// section, not here. Any other `observability.tracing.*` key (the removed
    /// subscriber knobs — `format`, `filter`, …) is recorded and reported by
    /// the plugin as one `ERROR` naming
    /// `docs/migration/observability-tracing.md`; boot continues.
    pub fn from_r2e_config(config: &r2e_core::R2eConfig, service_name: &str) -> Self {
        let mut cfg = Self::new(service_name);
        if let Ok(endpoint) = config.get::<String>("observability.otlp-endpoint") {
            cfg.otlp_endpoint = endpoint;
        }
        if let Ok(protocol) = config.get::<String>("observability.otlp-protocol")
            && let Some(protocol) = parse_otlp_protocol(&protocol)
        {
            cfg.otlp_protocol = protocol;
        }
        if let Ok(ratio) = config.get::<f64>("observability.sampling-ratio") {
            cfg.sampling_ratio = ratio.clamp(0.0, 1.0);
        }
        if let Ok(enabled) = config.get::<bool>("observability.tracing.enabled") {
            cfg.tracing_enabled = enabled;
        }
        cfg.leftover_tracing_keys = config
            .sub_keys("observability.tracing")
            .into_iter()
            .filter(|key| key != "enabled")
            .map(|key| format!("observability.tracing.{key}"))
            .collect();
        cfg
    }
}

impl ObservabilityConfig {
    /// The leftover `observability.tracing.*` keys (anything but `enabled`)
    /// [`from_r2e_config`](Self::from_r2e_config) found — exposed for tests.
    #[doc(hidden)]
    pub fn leftover_tracing_keys(&self) -> &[String] {
        &self.leftover_tracing_keys
    }
}

pub(crate) fn otlp_endpoint_from_env() -> Option<String> {
    non_empty_env("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
        .or_else(|| non_empty_env("OTEL_EXPORTER_OTLP_ENDPOINT"))
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn parse_otlp_protocol(value: &str) -> Option<OtlpProtocol> {
    match value.trim().to_ascii_lowercase().as_str() {
        "http" | "http/protobuf" => Some(OtlpProtocol::Http),
        "grpc" => Some(OtlpProtocol::Grpc),
        _ => None,
    }
}
