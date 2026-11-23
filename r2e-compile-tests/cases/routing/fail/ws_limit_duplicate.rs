//! Each `#[ws]` limit is given at most once.

use r2e::prelude::*;
use r2e::web::ws::WsStream;

#[controller(path = "/ws")]
pub struct ChatController;

#[routes]
impl ChatController {
    #[ws("/chat", max_message_size = 1024, max_message_size = 2048)]
    async fn chat(&self, _ws: WsStream) {}
}

fn main() {}
