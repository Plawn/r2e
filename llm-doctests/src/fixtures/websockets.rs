//! Scaffolding for `llm/websockets.md`.

/// The frame type `feed`/`send` take (`r2e::http::ws::Message`).
pub use r2e::http::ws::Message;

/// The session handle and its error type (`r2e::web::ws::…`).
pub use r2e::web::ws::{WsError, WsStream};
