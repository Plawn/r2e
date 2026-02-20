//! Built-in plugins for common cross-cutting concerns.
//!
//! Each plugin implements [`Plugin`](crate::plugin::Plugin) and can be
//! installed via [`AppBuilder::with()`](crate::builder::AppBuilder::with).

use crate::builder::AppBuilder;
use crate::plugin::Plugin;
use tower_http::cors::CorsLayer;

/// CORS plugin.
///
/// Use [`Cors::permissive()`] for a development-friendly configuration that
/// allows any origin, method, and header. Use [`Cors::custom()`] for a
/// production-ready configuration with a specific `CorsLayer`.
pub struct Cors {
    layer: CorsLayer,
}

impl Cors {
    /// Create a permissive CORS plugin (any origin, method, header).
    pub fn permissive() -> Self {
        Self {
            layer: crate::layers::default_cors(),
        }
    }

    /// Create a CORS plugin with a custom `CorsLayer`.
    pub fn custom(layer: CorsLayer) -> Self {
        Self { layer }
    }
}

impl Plugin for Cors {
    fn install<T: Clone + Send + Sync + 'static>(self, app: AppBuilder<T>) -> AppBuilder<T> {
        app.with_layer_fn(move |router| router.layer(self.layer))
    }
}

/// HTTP request/response tracing plugin.
///
/// Initialises the global `tracing` subscriber (via [`init_tracing()`]) and
/// adds a tower-http `TraceLayer` that logs requests and responses at the
/// `DEBUG` level.
///
/// This is the **lightweight** tracing option bundled with `r2e-core`. It
/// writes structured logs to stdout but does **not** export traces to an
/// external collector.
///
/// # When to use `Tracing` vs `Observability`
///
/// | | `Tracing` | `Observability` |
/// |---|---|---|
/// | Crate | `r2e-core` (always available) | `r2e-observability` (feature `observability`) |
/// | Log subscriber | `tracing_subscriber::fmt` | `tracing_subscriber::fmt` + `tracing-opentelemetry` |
/// | HTTP trace layer | tower-http `TraceLayer` | tower-http `TraceLayer` + `OtelTraceLayer` |
/// | Distributed tracing | No | Yes (OTLP export to Jaeger, Tempo, etc.) |
/// | Context propagation | No | Yes (W3C `traceparent`) |
/// | Configuration | None (RUST_LOG only) | `ObservabilityConfig` builder + YAML |
///
/// **Rule of thumb:** use `Tracing` for local development and simple services.
/// Switch to `Observability` when you need distributed tracing across
/// microservices.
///
/// **Do not** install both — `Observability` already includes the
/// `TraceLayer` and its own log subscriber.
///
/// [`init_tracing()`]: crate::init_tracing
///
/// # Example
///
/// ```ignore
/// AppBuilder::new()
///     .build_state::<MyState, _, _>()
///     .await
///     .with(Tracing)
///     .serve("0.0.0.0:3000")
///     .await;
/// ```
pub struct Tracing;

impl Plugin for Tracing {
    fn install<T: Clone + Send + Sync + 'static>(self, app: AppBuilder<T>) -> AppBuilder<T> {
        crate::layers::init_tracing();
        app.with_layer_fn(|router| router.layer(crate::layers::default_trace()))
    }
}

/// Health-check endpoint plugin.
///
/// # Simple mode (backwards-compatible)
///
/// ```ignore
/// .with(Health)  // GET /health → "OK"
/// ```
///
/// # Advanced mode
///
/// ```ignore
/// .with(Health::builder()
///     .check(DbHealth::new(pool.clone()))
///     .check(RedisHealth::new(redis.clone()))
///     .build())
/// ```
///
/// Advanced mode provides:
/// - `GET /health` → JSON with aggregated status (200/503)
/// - `GET /health/live` → always 200 (liveness probe)
/// - `GET /health/ready` → 200 if all checks pass, 503 otherwise
pub struct Health;

impl Health {
    /// Start building an advanced health check configuration.
    pub fn builder() -> crate::health::HealthBuilder {
        crate::health::HealthBuilder::new()
    }
}

impl Plugin for Health {
    fn install<T: Clone + Send + Sync + 'static>(self, app: AppBuilder<T>) -> AppBuilder<T> {
        app.register_routes(
            crate::http::Router::new()
                .route("/health", crate::http::routing::get(simple_health_handler)),
        )
    }
}

async fn simple_health_handler() -> &'static str {
    "OK"
}

/// Advanced health-check plugin with liveness/readiness probes.
///
/// Created via [`Health::builder()`].
pub struct AdvancedHealth {
    checks: Vec<Box<dyn crate::health::HealthIndicatorErased>>,
    cache_ttl: Option<std::time::Duration>,
}

impl AdvancedHealth {
    pub(crate) fn new(
        checks: Vec<Box<dyn crate::health::HealthIndicatorErased>>,
        cache_ttl: Option<std::time::Duration>,
    ) -> Self {
        Self { checks, cache_ttl }
    }
}

impl Plugin for AdvancedHealth {
    fn install<T: Clone + Send + Sync + 'static>(self, app: AppBuilder<T>) -> AppBuilder<T> {
        use std::sync::Arc;
        let state = Arc::new(crate::health::HealthState {
            checks: self.checks,
            start_time: std::time::Instant::now(),
            cache_ttl: self.cache_ttl,
            cache: tokio::sync::RwLock::new(None),
        });
        let s1 = state.clone();
        app.register_routes(
            crate::http::Router::new()
                .route(
                    "/health",
                    crate::http::routing::get(crate::health::health_handler)
                        .with_state(state),
                )
                .route(
                    "/health/live",
                    crate::http::routing::get(crate::health::liveness_handler),
                )
                .route(
                    "/health/ready",
                    crate::http::routing::get(crate::health::readiness_handler)
                        .with_state(s1),
                ),
        )
    }
}

/// Error-handling plugin.
///
/// Adds a `CatchPanicLayer` that converts panics into JSON 500 responses.
pub struct ErrorHandling;

impl Plugin for ErrorHandling {
    fn install<T: Clone + Send + Sync + 'static>(self, app: AppBuilder<T>) -> AppBuilder<T> {
        app.with_layer_fn(|router| router.layer(crate::layers::catch_panic_layer()))
    }
}

/// Dev-mode reload endpoints plugin.
///
/// Adds `/__r2e_dev/status` and `/__r2e_dev/ping` endpoints for
/// tooling and browser scripts to detect server restarts.
pub struct DevReload;

impl Plugin for DevReload {
    fn install<T: Clone + Send + Sync + 'static>(self, app: AppBuilder<T>) -> AppBuilder<T> {
        app.register_routes(crate::dev::dev_routes())
    }
}

/// Trailing-slash normalization plugin.
///
/// Removes trailing slashes from request paths, so `/users/` becomes `/users`.
/// This ensures consistent routing regardless of whether clients include
/// a trailing slash.
///
/// Uses a router fallback approach: when no route matches and the path has a
/// trailing slash, the request is re-dispatched with the slash stripped. This
/// can be installed at any point in the plugin chain.
pub struct NormalizePath;

impl Plugin for NormalizePath {
    fn install<T: Clone + Send + Sync + 'static>(self, app: AppBuilder<T>) -> AppBuilder<T> {
        app.enable_normalize_path()
    }
}
