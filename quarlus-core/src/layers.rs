use axum::http::StatusCode;
use axum::response::IntoResponse;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

/// Initialise the global `tracing` subscriber with a standard `fmt` layer.
///
/// Respects the `RUST_LOG` environment variable. Falls back to
/// `info,tower_http=debug` when `RUST_LOG` is not set.
///
/// Call this once, at the very start of `main`, before any tracing macro.
pub fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=debug".parse().unwrap()),
        )
        .init();
}

/// Returns a permissive CORS layer that allows any origin, method, and headers.
///
/// Suitable for development or internal services. For production, prefer
/// `AppBuilder::with_cors_config` with a stricter `CorsLayer`.
pub fn default_cors() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any)
}

/// Returns a `TraceLayer` configured for HTTP request/response tracing.
///
/// Uses `tower_http`'s default classification which logs at the `DEBUG` level
/// for requests and responses.
pub fn default_trace() -> TraceLayer<tower_http::classify::SharedClassifier<tower_http::classify::ServerErrorsAsFailures>> {
    TraceLayer::new_for_http()
}

/// Returns a `CatchPanicLayer` that converts panics into JSON 500 responses.
pub fn catch_panic_layer() -> CatchPanicLayer<fn(Box<dyn std::any::Any + Send>) -> axum::response::Response> {
    CatchPanicLayer::custom(panic_handler as fn(_) -> _)
}

fn panic_handler(_err: Box<dyn std::any::Any + Send>) -> axum::response::Response {
    let body = serde_json::json!({ "error": "Internal server error" });
    (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(body)).into_response()
}
