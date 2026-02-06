//! WebSocket re-exports from Axum.
//!
//! Gated behind the `ws` feature flag.

pub use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};

/// Marker trait for compile-time verification in the `#[ws]` macro.
pub trait IsWebSocket: Send + 'static {}

impl IsWebSocket for WebSocket {}
