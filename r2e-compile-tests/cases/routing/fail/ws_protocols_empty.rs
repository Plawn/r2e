//! An empty `protocols` list negotiates nothing — drop the argument instead.

use r2e::prelude::*;
use r2e::web::ws::WsStream;

#[controller(path = "/ws")]
pub struct ChatController;

#[routes]
impl ChatController {
    #[ws("/chat", protocols = [])]
    async fn chat(&self, _ws: WsStream) {}
}

fn main() {}
