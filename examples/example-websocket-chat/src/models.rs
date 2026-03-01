use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Incoming message from WebSocket client.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum WsIncoming {
    #[serde(rename = "message")]
    Message { text: String },
}

/// Outgoing message to WebSocket clients.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum WsOutgoing {
    #[serde(rename = "message")]
    Message {
        username: String,
        text: String,
        room: String,
    },
    #[serde(rename = "join")]
    Join { username: String, room: String },
    #[serde(rename = "leave")]
    Leave { username: String, room: String },
}

/// Event emitted when a message is sent, consumed for persistence.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MessageSentEvent {
    pub room: String,
    pub username: String,
    pub text: String,
}

/// Stored message in database history.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, sqlx::FromRow)]
pub struct StoredMessage {
    pub id: i64,
    pub room: String,
    pub username: String,
    pub text: String,
}
