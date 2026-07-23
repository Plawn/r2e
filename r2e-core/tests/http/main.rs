//! HTTP surface: extractors, response types, error mapping, the streaming
//! primitives (SSE/WebSocket), the managed-resource lifecycle, and the
//! built-in HTTP plugins.

#[path = "../support/mod.rs"]
mod support;

mod api_error;
mod error;
mod extract;
mod health;
mod managed;
#[cfg(feature = "multipart")]
mod multipart;
mod plugins;
mod request_id;
mod secure_headers;
mod sse;
#[cfg(feature = "ws")]
mod ws;
