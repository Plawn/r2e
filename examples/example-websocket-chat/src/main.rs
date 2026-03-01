use r2e::prelude::*;
use r2e::ws::WsRooms;
use sqlx::SqlitePool;

mod controllers;
mod models;
mod services;
mod state;

use controllers::chat_controller::ChatController;
use controllers::consumer::MessagePersistenceConsumer;
use controllers::history_controller::HistoryController;
use state::AppState;

#[r2e::main]
async fn main() {
    let config = R2eConfig::load("dev").unwrap_or_else(|_| R2eConfig::empty());
    let event_bus = LocalEventBus::new();
    let ws_rooms = WsRooms::new(128);

    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            room TEXT NOT NULL,
            username TEXT NOT NULL,
            text TEXT NOT NULL
        )",
    )
    .execute(&pool)
    .await
    .unwrap();

    AppBuilder::new()
        .provide(event_bus)
        .provide(ws_rooms)
        .provide(pool)
        .provide(config.clone())
        .with_bean::<services::ChatService>()
        .build_state::<AppState, _, _>()
        .await
        .with_config(config)
        .with(Health)
        .with(Cors::permissive())
        .with(Tracing)
        .with(ErrorHandling)
        .register_controller::<ChatController>()
        .register_controller::<HistoryController>()
        .register_controller::<MessagePersistenceConsumer>()
        .serve("0.0.0.0:3000")
        .await
        .unwrap();
}
