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
pub mod middleware;
pub mod propagation;
pub mod tracing_setup;

pub use client::{
    inject_current_context, traced_reqwest_client, DisableOtelPropagation, OtelName,
    OtelPathNames, R2eSpanBackend, TraceContextMiddleware,
};
pub use config::{ObservabilityConfig, OtlpProtocol, PropagationFormat};
// LogFormat is re-exported from r2e_core for backward compatibility.
pub use r2e_core::LogFormat;
pub use tracing_setup::OtelGuard;

use r2e_core::Plugin;

/// Full-stack observability plugin — OpenTelemetry tracing, context
/// propagation, and HTTP request logging.
///
/// This plugin is a **superset** of [`Tracing`](r2e_core::builtins::Tracing).
/// It replaces both `init_tracing()` and `.plugin(Tracing)` with a single call
/// that additionally exports distributed traces via OTLP.
///
/// # What it does
///
/// 1. Initialises a `tracing-subscriber` stack (fmt layer + OTel layer).
/// 2. Installs a W3C `traceparent` propagator for cross-service context.
/// 3. Adds a tower-http `TraceLayer` (same as the `Tracing` plugin).
/// 4. Adds an `OtelTraceLayer` that creates OTel spans for each HTTP request.
/// 5. Registers a shutdown hook that flushes pending traces on shutdown.
/// 6. Adds `trace_id` and `span_id` to logs emitted inside traced spans.
///
/// # When to use `Observability` vs `Tracing`
///
/// | | `Tracing` | `Observability` |
/// |---|---|---|
/// | Crate | `r2e-core` (always available) | `r2e-observability` (feature `observability`) |
/// | Log subscriber | `tracing_subscriber::fmt` | `tracing_subscriber::fmt` + `tracing-opentelemetry` |
/// | HTTP trace layer | tower-http `TraceLayer` | tower-http `TraceLayer` + `OtelTraceLayer` |
/// | Distributed tracing | No | Yes (OTLP export to Jaeger, Tempo, etc.) |
/// | Context propagation | No | Yes (W3C `traceparent`) |
/// | OTLP transport | — | HTTP/protobuf (normally port 4318) |
/// | Configuration | None (`RUST_LOG` only) | `ObservabilityConfig` builder + YAML |
///
/// **Do not** install both `Tracing` and `Observability` — this plugin
/// already includes everything `Tracing` provides.
///
/// # Example
///
/// ```rust,ignore
/// use r2e_observability::{Observability, ObservabilityConfig};
///
/// AppBuilder::new()
///     // No init_tracing() call needed — the plugin handles it
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
}

impl Observability {
    /// Create a new observability plugin with the given configuration.
    pub fn new(config: ObservabilityConfig) -> Self {
        Self {
            config,
            otlp_enabled: true,
        }
    }

    /// Create from R2eConfig (reads `observability.*` keys).
    pub fn from_config(r2e_config: &r2e_core::R2eConfig, service_name: &str) -> Self {
        Self {
            config: ObservabilityConfig::from_r2e_config(r2e_config, service_name),
            otlp_enabled: true,
        }
    }

    /// Configure observability from standard `OTEL_*` environment variables.
    ///
    /// When neither `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` nor
    /// `OTEL_EXPORTER_OTLP_ENDPOINT` is present, this installs the standard
    /// R2E tracing subscriber and HTTP trace layer without an OTLP exporter.
    pub fn from_env(service_name: &str) -> Self {
        let otlp_enabled = config::otlp_endpoint_from_env().is_some()
            && !matches!(
                std::env::var("OTEL_SDK_DISABLED").as_deref(),
                Ok("true") | Ok("TRUE") | Ok("1")
            );
        Self {
            config: ObservabilityConfig::from_env(service_name),
            otlp_enabled,
        }
    }

    /// Whether this plugin instance will export traces over OTLP.
    pub fn is_otlp_enabled(&self) -> bool {
        self.otlp_enabled
    }
}

impl Plugin for Observability {
    type Provided = ();
    type Deps = ();
    type Config = ();
    type Controllers = ();

    /// The global propagator and the `tracing` subscriber are process-wide,
    /// one-shot installs: they happen in `setup` (at `.plugin()` time) so bean
    /// construction and every other plugin's `build` are already instrumented.
    fn setup(&mut self, ctx: &mut r2e_core::plugin::PluginSetupContext) {
        if self.otlp_enabled {
            propagation::install_propagator(&self.config);
        }

        let guard = if self.config.tracing_enabled && self.otlp_enabled {
            Some(tracing_setup::init_tracing(&self.config))
        } else {
            if self.config.tracing_enabled {
                r2e_core::init_tracing_with_config(&self.config.tracing);
            }
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
        // tower-http TraceLayer (replaces the `Tracing` plugin) + OTel context
        // middleware.
        let capture_headers = self.config.capture_headers.clone();
        let otlp_enabled = self.otlp_enabled;
        ctx.add_layer(move |router| {
            let router = router.layer(r2e_core::runtime::layers::default_trace());
            if otlp_enabled {
                router.layer(middleware::OtelTraceLayer::new(capture_headers))
            } else {
                router
            }
        });

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
