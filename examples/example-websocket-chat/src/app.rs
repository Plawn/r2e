// Canonical example-websocket-chat application source.
//
// `lib.rs` includes this file so the app can be booted by type; `app_main!`
// includes the same file directly in the binary tip crate for production and
// real Subsecond hot-patching.

use r2e::prelude::*;
use r2e::ws::WsRooms;
use sqlx::SqlitePool;

pub mod controllers;
pub mod models;
pub mod services;

use controllers::chat_controller::ChatController;
use controllers::consumer::MessagePersistenceConsumer;
use controllers::history_controller::HistoryController;

/// Resources provisioned once by [`App::setup`]; in dev mode they survive
/// hot-patches.
#[derive(Clone)]
pub struct AppEnv {
    event_bus: LocalEventBus,
    ws_rooms: WsRooms,
    pool: SqlitePool,
}

/// The canonical application blueprint.
pub struct ChatApp;

impl App for ChatApp {
    type Env = AppEnv;

    async fn setup() -> AppEnv {
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

        AppEnv {
            event_bus,
            ws_rooms,
            pool,
        }
    }

    async fn build(b: AppBuilder, env: AppEnv) -> impl BootableApp {
        b.load_config::<()>()
            .provide(env.event_bus)
            .provide(env.ws_rooms)
            .provide(env.pool)
            .register::<services::ChatService>()
            .build_state()
            .await
            .with(Health)
            .with(Cors::permissive())
            .with(Tracing)
            .with(ErrorHandling)
            .register_controllers::<(ChatController, HistoryController, MessagePersistenceConsumer)>()
    }
}
