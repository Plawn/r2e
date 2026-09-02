//! Scaffolding for `llm/sse.md`.

use serde::{Deserialize, Serialize};

/// The typed event carried by the `SseTopic<E>` the controller injects.
#[derive(Clone, Serialize, Deserialize)]
pub struct UserCreatedEvent {
    pub user_id: i64,
}
