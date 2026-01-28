//! Built-in plugins for common cross-cutting concerns.
//!
//! Each plugin implements [`Plugin<T>`](crate::plugin::Plugin) and can be
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

impl<T: Clone + Send + Sync + 'static> Plugin<T> for Cors {
    fn install(self, app: AppBuilder<T>) -> AppBuilder<T> {
        app.with_layer_fn(move |router| router.layer(self.layer))
    }
}

/// HTTP request/response tracing plugin.
///
/// Adds a `TraceLayer` that logs requests and responses at the `DEBUG` level.
pub struct Tracing;

impl<T: Clone + Send + Sync + 'static> Plugin<T> for Tracing {
    fn install(self, app: AppBuilder<T>) -> AppBuilder<T> {
        app.with_layer_fn(|router| router.layer(crate::layers::default_trace()))
    }
}

/// Health-check endpoint plugin.
///
/// Adds a `GET /health` endpoint that returns `"OK"` with status 200.
pub struct Health;

impl<T: Clone + Send + Sync + 'static> Plugin<T> for Health {
    fn install(self, app: AppBuilder<T>) -> AppBuilder<T> {
        app.register_routes(
            crate::http::Router::new()
                .route("/health", crate::http::routing::get(health_handler)),
        )
    }
}

async fn health_handler() -> &'static str {
    "OK"
}

/// Error-handling plugin.
///
/// Adds a `CatchPanicLayer` that converts panics into JSON 500 responses.
pub struct ErrorHandling;

impl<T: Clone + Send + Sync + 'static> Plugin<T> for ErrorHandling {
    fn install(self, app: AppBuilder<T>) -> AppBuilder<T> {
        app.with_layer_fn(|router| router.layer(crate::layers::catch_panic_layer()))
    }
}

/// Dev-mode reload endpoints plugin.
///
/// Adds `/__quarlus_dev/status` and `/__quarlus_dev/ping` endpoints for
/// tooling and browser scripts to detect server restarts.
pub struct DevReload;

impl<T: Clone + Send + Sync + 'static> Plugin<T> for DevReload {
    fn install(self, app: AppBuilder<T>) -> AppBuilder<T> {
        app.register_routes(crate::dev::dev_routes())
    }
}

/// Trailing-slash normalization plugin.
///
/// Removes trailing slashes from request paths, so `/users/` becomes `/users`.
/// This ensures consistent routing regardless of whether clients include
/// a trailing slash.
pub struct NormalizePath;

impl<T: Clone + Send + Sync + 'static> Plugin<T> for NormalizePath {
    fn install(self, app: AppBuilder<T>) -> AppBuilder<T> {
        use tower_http::normalize_path::NormalizePathLayer;
        app.with_layer_fn(|router| router.layer(NormalizePathLayer::trim_trailing_slash()))
    }
}
