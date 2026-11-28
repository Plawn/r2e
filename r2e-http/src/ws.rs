pub use axum::extract::ws::{
    rejection::WebSocketUpgradeRejection, CloseFrame, Message, WebSocket, WebSocketUpgrade,
};

/// Whether a receive error means the peer sent a message or a frame larger
/// than the session accepts (`max_message_size` / `max_frame_size`).
///
/// The backend reports every receive failure as one opaque [`Error`](crate::Error);
/// this reads the protocol error underneath it. `r2e-core` uses it to answer
/// with a `1009 Message Too Big` close frame instead of dropping the socket.
#[must_use]
pub fn is_message_too_big(err: &crate::Error) -> bool {
    let mut source = std::error::Error::source(err);
    while let Some(cause) = source {
        if let Some(tungstenite::Error::Capacity(_)) = cause.downcast_ref::<tungstenite::Error>() {
            return true;
        }
        source = cause.source();
    }
    false
}
