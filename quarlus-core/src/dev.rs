//! Dev-mode support endpoints.
//!
//! When enabled via `AppBuilder::with_dev_reload()`, the server exposes:
//! - `GET /__quarlus_dev/status` — Returns `"dev"` so tooling/scripts can
//!   detect that the server is running in dev mode.
//! - `GET /__quarlus_dev/ping` — Returns a timestamp; can be polled by a
//!   browser script to detect when the server has restarted (the PID or
//!   boot-time changes).
//!
//! Pair with `quarlus-cli dev` (which wraps `cargo-watch`) for a full
//! hot-reload development experience. When cargo-watch detects a file
//! change, it kills the server and restarts it. Clients polling
//! `/__quarlus_dev/ping` detect the restart and refresh.

use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
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
/// Intended to be merged into the main application via
/// `AppBuilder::with_dev_reload()`.
pub fn dev_routes<T: Clone + Send + Sync + 'static>() -> Router<T> {
    Router::new()
        .route("/__quarlus_dev/status", get(status_handler))
        .route("/__quarlus_dev/ping", get(ping_handler))
}

async fn status_handler() -> impl IntoResponse {
    "dev"
}

async fn ping_handler() -> impl IntoResponse {
    let ts = boot_time();
    serde_json::json!({ "boot_time": ts, "status": "ok" }).to_string()
}
