//! Shared test helpers for the `r2e-core` test targets.
//!
//! This file is **not** a test target of its own (no `main.rs` in the
//! directory, so Cargo ignores it). Each target pulls it in with:
//!
//! ```ignore
//! #[path = "../support/mod.rs"]
//! mod support;
//! ```
//!
//! Only put things here that at least two targets need. Fixtures specific to
//! one subsystem belong in that subsystem's module.

#![allow(dead_code)]

use http_body_util::BodyExt;
use r2e_core::http::{Body, Request, Response, Router, StatusCode};
use tower::ServiceExt;

// ── Router driving (`tower::oneshot`) ──────────────────────────────────────

/// Collect a response body into a `String` (lossy — test bodies are UTF-8).
pub async fn body_string(resp: Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).to_string()
}

/// Drive one request through the router and return the raw response.
pub async fn raw(
    router: Router,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Body,
) -> Response {
    let mut b = Request::builder().method(method).uri(path);
    for (name, value) in headers {
        b = b.header(*name, *value);
    }
    router.oneshot(b.body(body).unwrap()).await.unwrap()
}

/// Drive one request through the router and return `(status, body)`.
pub async fn send(
    router: Router,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Body,
) -> (StatusCode, String) {
    let resp = raw(router, method, path, headers, body).await;
    let status = resp.status();
    (status, body_string(resp).await)
}

/// `GET path` → `(status, body)`.
pub async fn send_get(router: Router, path: &str) -> (StatusCode, String) {
    send(router, "GET", path, &[], Body::empty()).await
}

/// `GET path` with headers → `(status, body)`.
pub async fn send_get_with(
    router: Router,
    path: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, String) {
    send(router, "GET", path, headers, Body::empty()).await
}

/// `GET path` with headers → raw response (for header/extension assertions).
pub async fn raw_get_with(router: Router, path: &str, headers: &[(&str, &str)]) -> Response {
    raw(router, "GET", path, headers, Body::empty()).await
}

// ── Config fixtures ────────────────────────────────────────────────────────

/// Write `content` to `<dir>/<name>` and return the full path.
pub fn write_file(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

// ── Process environment ────────────────────────────────────────────────────

/// Serializes tests that mutate `std::env`.
///
/// Tests within one target share a process, and `setenv` racing a `getenv` on
/// another thread is unsound — so every test that sets or removes a variable
/// must hold this guard for the whole test, even when the variable names do
/// not overlap.
pub fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
