//! Dev-mode support endpoints.
//!
//! When enabled via `.with(DevReload)`, the server exposes:
//! - `GET /__r2e_dev/status` — Returns `"dev"` so tooling/scripts can
//!   detect that the server is running in dev mode.
//! - `GET /__r2e_dev/ping` — Returns a timestamp; can be polled by a
//!   browser script to detect when the server has restarted (the PID or
//!   boot-time changes).
//!
//! Pair with `r2e-cli dev` (which wraps `cargo-watch`) for a full
//! hot-reload development experience. When cargo-watch detects a file
//! change, it kills the server and restarts it. Clients polling
//! `/__r2e_dev/ping` detect the restart and refresh.

use crate::http::header::{HeaderValue, CACHE_CONTROL};
use crate::http::middleware::Next;
use crate::http::response::IntoResponse;
use crate::http::routing::get;
use crate::http::Router;
use axum::extract::Request;
use axum::http::header::CONNECTION;
use axum::response::Response;
use std::sync::OnceLock;
use std::time::SystemTime;

static BOOT_TIME: OnceLock<u64> = OnceLock::new();

fn boot_time() -> u64 {
    *BOOT_TIME.get_or_init(|| {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    })
}

/// Create a router with dev-mode endpoints.
///
/// Intended to be merged into the main application via the
/// [`DevReload`](crate::plugins::DevReload) plugin.
pub fn dev_routes<T: Clone + Send + Sync + 'static>() -> Router<T> {
    Router::new()
        .route("/__r2e_dev/status", get(status_handler))
        .route("/__r2e_dev/ping", get(ping_handler))
}

async fn status_handler() -> impl IntoResponse {
    "dev"
}

async fn ping_handler() -> impl IntoResponse {
    let ts = boot_time();
    serde_json::json!({ "boot_time": ts, "status": "ok" }).to_string()
}

/// Middleware that adds dev-mode headers to every response:
///
/// - `Cache-Control: no-store` — prevents the browser from caching API
///   responses, so Swagger UI always shows fresh data.
/// - `Connection: close` — forces the browser to close the TCP connection
///   after each response. Without this, HTTP keep-alive lets the browser
///   reuse a connection bound to a *previous* server future. When subsecond
///   hot-patches, it drops the old server and starts a new one, but the old
///   connection handler tasks (spawned via `tokio::spawn`) keep running.
///   The browser's keep-alive connection stays routed to stale handlers.
pub async fn dev_headers_middleware(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(CONNECTION, HeaderValue::from_static("close"));
    response
}
