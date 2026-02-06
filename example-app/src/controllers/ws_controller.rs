use quarlus::prelude::*;
use quarlus::ws::WsStream;

use crate::state::Services;

#[derive(Controller)]
#[controller(path = "/ws", state = Services)]
pub struct WsEchoController;

#[routes]
impl WsEchoController {
    /// WebSocket echo endpoint — echoes back any message received.
    #[ws("/echo")]
    async fn echo(&self, mut ws: WsStream) {
        ws.send_text("Welcome to the echo server!").await.ok();
        ws.on_each(|msg| async move { Some(msg) }).await;
    }
}
