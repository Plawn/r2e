//! `#[ws]` takes a path and, optionally, `protocols = [..]` — nothing else.

use r2e::prelude::*;
use r2e::web::ws::WsStream;

#[controller(path = "/ws")]
pub struct ChatController;

#[routes]
impl ChatController {
    #[ws("/chat", subprotocols = ["chat.v1"])]
    async fn chat(&self, _ws: WsStream) {}
}

fn main() {}
