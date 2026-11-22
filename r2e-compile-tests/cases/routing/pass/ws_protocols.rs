//! `#[ws("/path", protocols = [..])]` takes `&'static str` expressions —
//! literals and constants alike.

use r2e::prelude::*;
use r2e::web::ws::WsStream;

const V2: &str = "chat.v2";

#[controller(path = "/ws")]
pub struct ChatController;

#[routes]
impl ChatController {
    #[ws("/chat", protocols = [V2, "chat.v1"])]
    async fn chat(&self, mut ws: WsStream) {
        let negotiated = ws.protocol().unwrap_or("none").to_string();
        ws.send_text(negotiated).await.ok();
    }
}

fn main() {}
