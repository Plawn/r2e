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
//!     .build_state::<MyState, _>()
//!     .await
//!     .with(Observability::new(
//!         ObservabilityConfig::new("my-service")
//!             .with_service_version("1.0.0")
//!             .with_endpoint("http://otel-collector:4317")
//!             .capture_header("x-tenant-id"),
//!     ))
//!     .serve("0.0.0.0:3000")
//!     .await;
//! ```

pub mod config;
pub mod middleware;
pub mod propagation;
pub mod tracing_setup;

pub use config::{LogFormat, ObservabilityConfig, OtlpProtocol, PropagationFormat};
pub use tracing_setup::OtelGuard;

use r2e_core::Plugin;

/// Full-stack observability plugin — OpenTelemetry tracing, context
/// propagation, and HTTP request logging.
///
/// This plugin is a **superset** of [`Tracing`](r2e_core::plugins::Tracing).
/// It replaces both `init_tracing()` and `.with(Tracing)` with a single call
/// that additionally exports distributed traces via OTLP.
///
/// # What it does
///
/// 1. Initialises a `tracing-subscriber` stack (fmt layer + OTel layer).
/// 2. Installs a W3C `traceparent` propagator for cross-service context.
/// 3. Adds a tower-http `TraceLayer` (same as the `Tracing` plugin).
/// 4. Adds an `OtelTraceLayer` that creates OTel spans for each HTTP request.
/// 5. Registers an `on_stop` hook that flushes pending traces on shutdown.
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
///     .build_state::<MyState, _>()
///     .await
///     // No init_tracing() call needed — the plugin handles it
///     .with(Observability::new(
///         ObservabilityConfig::new("my-service")
///             .with_service_version("1.0.0")
///             .with_endpoint("http://otel-collector:4317"),
///     ))
///     .serve("0.0.0.0:3000")
///     .await;
/// ```
pub struct Observability {
    config: ObservabilityConfig,
}

impl Observability {
    /// Create a new observability plugin with the given configuration.
    pub fn new(config: ObservabilityConfig) -> Self {
        Self { config }
    }

    /// Create from R2eConfig (reads `observability.*` keys).
    pub fn from_config(r2e_config: &r2e_core::R2eConfig, service_name: &str) -> Self {
        Self {
            config: ObservabilityConfig::from_r2e_config(r2e_config, service_name),
        }
    }
}

impl Plugin for Observability {
    fn install<T: Clone + Send + Sync + 'static>(
        self,
        app: r2e_core::AppBuilder<T>,
    ) -> r2e_core::AppBuilder<T> {
        // 1. Install global propagator
        propagation::install_propagator(&self.config);

        // 2. Initialize tracing + OTel (if enabled)
        let guard = if self.config.tracing_enabled {
            Some(tracing_setup::init_tracing(&self.config))
        } else {
            None
        };

        // 3. Add tower-http TraceLayer (replaces the Tracing plugin) + OTel context middleware
        let capture_headers = self.config.capture_headers.clone();
        let app = app.with_layer_fn(move |router| {
            router
                .layer(r2e_core::layers::default_trace())
                .layer(middleware::OtelTraceLayer::new(capture_headers))
        });

        // 4. Store the guard so it lives for the app lifetime; flush on stop
        if let Some(guard) = guard {
            let guard = std::sync::Arc::new(std::sync::Mutex::new(Some(guard)));
            let guard_clone = guard.clone();
            app.on_stop(move || async move {
                let _ = guard_clone.lock().unwrap().take();
                tracing::info!("OpenTelemetry traces flushed");
            })
        } else {
            app
        }
    }
}
