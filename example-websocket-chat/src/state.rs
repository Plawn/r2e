use r2e::prelude::*;
use r2e::ws::WsRooms;

use crate::services::ChatService;

#[derive(Clone, BeanState)]
pub struct AppState {
    pub chat_service: ChatService,
    pub ws_rooms: WsRooms,
    pub event_bus: EventBus,
    pub pool: sqlx::SqlitePool,
    pub config: R2eConfig,
}
