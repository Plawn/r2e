//! WebSocket re-exports from r2e-http.
//!
//! Gated behind the `ws` feature flag.

pub use r2e_http::ws::*;

/// Marker trait for compile-time verification in the `#[ws]` macro.
pub trait IsWebSocket: Send + 'static {}

impl IsWebSocket for WebSocket {}
