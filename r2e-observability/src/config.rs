use r2e_core::TracingConfig;

/// Configuration for the observability stack.
#[derive(Debug, Clone)]
pub struct ObservabilityConfig {
    /// Service name reported to the tracing backend.
    pub service_name: String,
    /// Service version (used in resource attributes).
    pub service_version: Option<String>,
    /// OTLP exporter endpoint (default: "http://localhost:4317").
    pub otlp_endpoint: String,
    /// Protocol: Grpc (default) or Http.
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
    /// Tracing subscriber configuration (format, filter, etc.).
    pub tracing: TracingConfig,
}

/// OTLP transport protocol.
#[derive(Debug, Clone, Default)]
pub enum OtlpProtocol {
    #[default]
    Grpc,
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
            otlp_endpoint: "http://localhost:4317".to_string(),
            otlp_protocol: OtlpProtocol::Grpc,
            tracing_enabled: true,
            sampling_ratio: 1.0,
            propagation_format: PropagationFormat::W3c,
            resource_attributes: Vec::new(),
            capture_headers: Vec::new(),
            tracing: TracingConfig::default(),
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

    /// Set the tracing subscriber configuration.
    pub fn with_tracing_config(mut self, config: TracingConfig) -> Self {
        self.tracing = config;
        self
    }

    /// Convenience: set the log format on the embedded tracing config.
    pub fn with_log_format(mut self, format: r2e_core::LogFormat) -> Self {
        self.tracing = self.tracing.with_format(format);
        self
    }

    pub fn disable_tracing(mut self) -> Self {
        self.tracing_enabled = false;
        self
    }

    /// Load from R2eConfig with prefix `observability`.
    ///
    /// Reads keys like:
    /// - `observability.otlp-endpoint`
    /// - `observability.sampling-ratio`
    /// - `observability.tracing.enabled`
    ///
    /// The embedded `TracingConfig` is loaded from `observability.tracing.*`.
    pub fn from_r2e_config(config: &r2e_core::R2eConfig, service_name: &str) -> Self {
        use r2e_core::ConfigProperties;

        let mut cfg = Self::new(service_name);
        if let Ok(endpoint) = config.get::<String>("observability.otlp-endpoint") {
            cfg.otlp_endpoint = endpoint;
        }
        if let Ok(ratio) = config.get::<f64>("observability.sampling-ratio") {
            cfg.sampling_ratio = ratio.clamp(0.0, 1.0);
        }
        if let Ok(enabled) = config.get::<bool>("observability.tracing.enabled") {
            cfg.tracing_enabled = enabled;
        }
        // Load the tracing subscriber config from observability.tracing.*
        if let Ok(tracing) = TracingConfig::from_config(config, Some("observability.tracing")) {
            cfg.tracing = tracing;
        }
        cfg
    }
}
