//! A limit is a size in bytes: `usize`, from an expression or a method.

use r2e::prelude::*;
use r2e::web::ws::WsStream;

#[controller(path = "/ws")]
pub struct ChatController;

#[routes]
impl ChatController {
    fn message_limit(&self) -> String {
        String::from("1024")
    }

    #[ws("/chat", max_message_size = "message_limit")]
    async fn chat(&self, _ws: WsStream) {}

    #[ws("/voice", max_frame_size = 1.5)]
    async fn voice(&self, _ws: WsStream) {}
}

fn main() {}
