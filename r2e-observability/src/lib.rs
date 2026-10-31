//! OpenTelemetry observability plugin for R2E.
//!
//! Provides distributed tracing via OpenTelemetry, context propagation,
//! and a Tower middleware layer for automatic span creation.
//!
//! # Usage
//!
//! ```rust,ignore
//! use r2e_observability::{Observability, ObservabilityConfig};
//!
//! AppBuilder::new()
//!     .plugin(Observability::new(
//!         ObservabilityConfig::new("my-service")
//!             .with_service_version("1.0.0")
//!             .with_endpoint("http://otel-collector:4318/v1/traces")
//!             .capture_header("x-tenant-id"),
//!     ))
//!     .build_state()
//!     .await
//!     .serve("0.0.0.0:3000")
//!     .await;
//! ```

pub mod client;
pub mod config;
pub mod propagation;
pub mod span;
pub mod tracing_setup;

pub use client::{
    inject_current_context, traced_reqwest_client, DisableOtelPropagation, OtelName, OtelPathNames,
    R2eSpanBackend, TraceContextMiddleware,
};
pub use config::{ObservabilityConfig, OtlpProtocol, PropagationFormat};
pub use span::OtelRequestSpan;
// LogFormat is re-exported from r2e_core for backward compatibility.
pub use r2e_core::LogFormat;
pub use tracing_setup::OtelGuard;

use r2e_core::{HttpTraceConfig, Plugin};

/// Full-stack observability plugin — OpenTelemetry tracing, context
/// propagation, and HTTP request logging.
///
/// The plugin does **not** build a `tracing` subscriber: the process has one,
/// installed by R2E from the app's top-level `tracing:` section, and this
/// plugin **contributes** to it.
///
/// # What it does
///
/// 1. Installs a W3C `traceparent` propagator for cross-service context.
/// 2. Contributes the `tracing-opentelemetry` layer to R2E's subscriber
///    ([`PluginSetupContext::tracing_layer`](r2e_core::plugin::PluginSetupContext::tracing_layer)),
///    so spans are exported over OTLP — `Suppress` rules and the
///    `RUST_LOG`/`tracing.filter` filter apply to the export too.
/// 3. Contributes [`OtelIds`](tracing_setup::OtelIds), which adds `trace_id`
///    and `span_id` to log lines emitted inside traced spans
///    ([`LogFields`](r2e_core::LogFields)).
/// 4. Installs `r2e-core`'s [`HttpTraceLayer`](r2e_core::HttpTraceLayer) with
///    the [`OtelRequestSpan`] shape — **one** span per request, semconv field
///    names, parent context from the inbound `traceparent`.
/// 5. Registers a shutdown hook that flushes pending traces on shutdown.
///
/// `build_state()` installs the subscriber with the contributed layers. When
/// the application installed its **own** subscriber first (or set
/// `tracing.enabled: false`), the layers are not part of it and nothing is
/// exported — add [`otel_layer`](tracing_setup::otel_layer) and
/// [`OtelIds`](tracing_setup::OtelIds) to that subscriber yourself.
///
/// # Who owns what
///
/// | | `build_state()` | `HttpTrace` | `Observability` |
/// |---|---|---|---|
/// | Crate | `r2e-core` | `r2e-core` | `r2e-observability` (feature `observability`) |
/// | Log subscriber | installs R2E's (`tracing.*`) | — | contributes to R2E's (OTel layer + `trace_id`/`span_id` fields) |
/// | Per-request span | — | `request` (short field names) | `request` (OTel semconv names) |
/// | Distributed tracing | No | No | Yes (OTLP export to Jaeger, Tempo, …) |
/// | Context propagation | No | No | Yes (W3C `traceparent`) |
/// | Configuration | `tracing.*` | `trace.*` | `observability.*` (+ `trace.*` for the span, `tracing.*` for the console) |
///
/// `HttpTrace` is redundant: `Observability`
/// installs that layer itself (installing both means two spans per request),
/// but it *reads the same `trace:` section*, so exclusions, request-id and
/// `record-path` are configured in exactly one place either way.
///
/// # Example
///
/// ```rust,ignore
/// use r2e_observability::{Observability, ObservabilityConfig};
///
/// AppBuilder::new()
///     // The console output comes from the top-level `tracing:` section.
///     .plugin(Observability::new(
///         ObservabilityConfig::new("my-service")
///             .with_service_version("1.0.0")
///             .with_endpoint("http://otel-collector:4318/v1/traces"),
///     ))
///     .build_state()
///     .await
///     .serve("0.0.0.0:3000")
///     .await;
/// ```
pub struct Observability {
    config: ObservabilityConfig,
    otlp_enabled: bool,
    /// Test seam: replaces the OTLP exporter (see [`Self::with_span_exporter`]).
    exporter: Option<ExporterOverride>,
}

type ExporterOverride = Box<dyn FnOnce(&ObservabilityConfig) -> OtelGuard + Send>;

impl Observability {
    /// Create a new observability plugin with the given configuration.
    pub fn new(config: ObservabilityConfig) -> Self {
        Self {
            config,
            otlp_enabled: true,
            exporter: None,
        }
    }

    /// Create from R2eConfig (reads `observability.*` keys).
    pub fn from_config(r2e_config: &r2e_core::R2eConfig, service_name: &str) -> Self {
        Self {
            config: ObservabilityConfig::from_r2e_config(r2e_config, service_name),
            otlp_enabled: true,
            exporter: None,
        }
    }

    /// Configure observability from standard `OTEL_*` environment variables.
    ///
    /// When neither `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` nor
    /// `OTEL_EXPORTER_OTLP_ENDPOINT` is present, the plugin only installs the
    /// HTTP trace layer (and R2E's subscriber, if nothing installed it yet):
    /// no OTLP export.
    pub fn from_env(service_name: &str) -> Self {
        let otlp_enabled = config::otlp_endpoint_from_env().is_some()
            && !matches!(
                std::env::var("OTEL_SDK_DISABLED").as_deref(),
                Ok("true") | Ok("TRUE") | Ok("1")
            );
        Self {
            config: ObservabilityConfig::from_env(service_name),
            otlp_enabled,
            exporter: None,
        }
    }

    /// Whether this plugin instance will export traces over OTLP.
    pub fn is_otlp_enabled(&self) -> bool {
        self.otlp_enabled
    }

    /// Export through `exporter` (synchronously, one export per span end)
    /// instead of OTLP — the seam tests use to read exported spans
    /// (`opentelemetry_sdk::trace::InMemorySpanExporter`).
    #[doc(hidden)]
    pub fn with_span_exporter<E>(mut self, exporter: E) -> Self
    where
        E: opentelemetry_sdk::trace::SpanExporter + 'static,
    {
        self.exporter = Some(Box::new(move |config: &ObservabilityConfig| {
            tracing_setup::init_tracing_with_exporter(config, exporter)
        }));
        self
    }
}

impl Plugin for Observability {
    type Provided = ();
    type Deps = ();
    type Config = ();
    type Controllers = ();

    /// The global propagator and the contributions to the `tracing`
    /// subscriber happen in `setup` (at `.plugin()` time): the layers are
    /// installed by `build_state()`, before bean construction and every
    /// plugin's `build`, so those are already instrumented.
    fn setup(&mut self, ctx: &mut r2e_core::plugin::PluginSetupContext) {
        if self.otlp_enabled {
            propagation::install_propagator(&self.config);
        }

        if !self.config.leftover_tracing_keys.is_empty() {
            tracing::error!(
                keys = ?self.config.leftover_tracing_keys,
                "r2e-observability: `observability.tracing.*` no longer configures the log \
                 subscriber and these keys are ignored — move them to the top-level `tracing:` \
                 section (see docs/migration/tracing-single-path.md). Only \
                 `observability.tracing.enabled` is still read."
            );
        }

        let guard = if self.config.tracing_enabled && self.otlp_enabled {
            let guard = match self.exporter.take() {
                Some(init) => init(&self.config),
                None => tracing_setup::init_tracing(&self.config),
            };
            // The OpenTelemetry layer first: `OtelIds` reads the context it
            // activates on span enter.
            ctx.tracing_layer(tracing_setup::otel_layer());
            ctx.tracing_layer(tracing_setup::OtelIds);
            Some(guard)
        } else {
            None
        };
        // Park the guard where `build` can pick it up (it is not `Clone`, and
        // `setup` cannot register effects).
        ctx.store_data(OtelGuardSlot(std::sync::Mutex::new(guard)));
    }

    async fn build(
        self,
        _deps: Self::Deps,
        _config: Option<Self::Config>,
        ctx: &mut r2e_core::plugin::PluginBuildContext,
    ) -> Result<Self::Provided, r2e_core::plugin::PluginBuildError> {
        // The one HTTP tracing layer: `r2e-core`'s, with the OTel span shape.
        // The knobs come from the app's `trace:` section — the same one
        // `HttpTrace` reads — with this plugin's `capture_headers` filling the
        // preset slot, so `trace.capture-headers` still wins in YAML.
        let raw = ctx.config_raw();
        let trace_enabled = raw
            .map(|cfg| cfg.get::<bool>("trace.enabled").unwrap_or(true))
            .unwrap_or(true);
        let file = raw
            .map(|cfg| {
                <HttpTraceConfig as r2e_core::config::PluginConfig>::plugin_load(cfg, "trace")
            })
            .transpose()?;
        let preset = HttpTraceConfig {
            capture_headers: (!self.config.capture_headers.is_empty())
                .then(|| self.config.capture_headers.clone()),
            ..HttpTraceConfig::default()
        };
        let settings = r2e_core::builtins::http_trace::resolve_settings(
            HttpTraceConfig::default(),
            file,
            Some(preset),
        )?;

        if trace_enabled {
            let make_span = OtelRequestSpan::new(std::sync::Arc::clone(&settings.capture_headers));
            ctx.add_layer(move |router| {
                router.layer(r2e_core::HttpTraceLayer::with_make_span(
                    settings, make_span,
                ))
            });
        }

        // Keep the tracer-provider guard alive for the app lifetime and drop
        // it (flushing spans) at shutdown.
        ctx.after_build(|dctx| {
            let Some(slot) = dctx.take_data::<OtelGuardSlot>() else {
                return;
            };
            if slot
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_none()
            {
                return;
            }
            let slot = std::sync::Arc::new(slot);
            dctx.on_shutdown_async(move || {
                let slot = std::sync::Arc::clone(&slot);
                async move {
                    let _ = slot
                        .0
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take();
                    tracing::info!("OpenTelemetry traces flushed");
                }
            });
        });

        Ok(())
    }
}

/// Carries the non-`Clone` OTel tracer-provider guard from `setup` to `build`.
struct OtelGuardSlot(std::sync::Mutex<Option<tracing_setup::OtelGuard>>);
