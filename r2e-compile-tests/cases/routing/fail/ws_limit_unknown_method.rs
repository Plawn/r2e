//! A string limit names a plain method of the same impl block.

use r2e::prelude::*;
use r2e::web::ws::WsStream;

#[controller(path = "/ws")]
pub struct ChatController;

#[routes]
impl ChatController {
    #[ws("/chat", max_message_size = "message_limit")]
    async fn chat(&self, _ws: WsStream) {}
}

fn main() {}
