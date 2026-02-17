use r2e::prelude::*;

use crate::models::StoredMessage;
use crate::services::ChatService;
use crate::state::AppState;

/// REST endpoints for room listing and message history.
#[derive(Controller)]
#[controller(path = "/rooms", state = AppState)]
pub struct HistoryController {
    #[inject]
    chat_service: ChatService,
}

#[routes]
impl HistoryController {
    /// List rooms that have messages.
    #[get("/")]
    async fn list_rooms(&self) -> Result<Json<Vec<String>>, AppError> {
        let rooms = self.chat_service.list_rooms().await?;
        Ok(Json(rooms))
    }

    /// Get message history for a room (last 50 messages).
    #[get("/{room}/history")]
    async fn room_history(
        &self,
        Path(room): Path<String>,
    ) -> Result<Json<Vec<StoredMessage>>, AppError> {
        let messages = self.chat_service.get_history(&room, 50).await?;
        Ok(Json(messages))
    }
}
