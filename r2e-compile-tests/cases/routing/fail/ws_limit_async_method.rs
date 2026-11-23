//! The limit provider is called right before the handshake: it must be sync.

use r2e::prelude::*;
use r2e::web::ws::WsStream;

#[controller(path = "/ws")]
pub struct ChatController;

#[routes]
impl ChatController {
    async fn message_limit(&self) -> usize {
        1024
    }

    #[ws("/chat", max_frame_size = "message_limit")]
    async fn chat(&self, _ws: WsStream) {}
}

fn main() {}
