//! `#[ws("/path", max_message_size = .., max_frame_size = ..)]`: each limit is
//! a `usize` expression or the name of a plain `&self -> usize` method, and
//! both combine with `protocols`.

use r2e::prelude::*;
use r2e::web::ws::WsStream;

const VOICE_FRAME: usize = 64 * 1024;

#[controller(path = "/ws")]
pub struct ChatController {
    #[config("collab.max-message-size")]
    max_message_size: usize,
}

#[routes]
impl ChatController {
    fn message_limit(&self) -> usize {
        self.max_message_size
    }

    #[ws("/voice", max_message_size = VOICE_FRAME, max_frame_size = 64 * 1024)]
    async fn voice(&self, _ws: WsStream) {}

    #[ws("/collab", protocols = ["collab.v1"], max_message_size = "message_limit")]
    async fn collab(&self, _ws: WsStream) {}

    #[ws("/frames", max_frame_size = "message_limit")]
    async fn frames(&self, _ws: WsStream) {}
}

fn main() {}
