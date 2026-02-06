use std::convert::Infallible;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use quarlus::prelude::*;
use quarlus::http::response::SseEvent;
use quarlus::http::ws::Message;
use quarlus::ws::WsStream;

use crate::services::NotificationService;
use crate::state::Services;

#[derive(Controller)]
#[controller(path = "/notifications", state = Services)]
pub struct NotificationController {
    #[inject]
    notification_service: NotificationService,
}

#[routes]
impl NotificationController {
    #[sse("/sse/{user_id}")]
    async fn sse_subscribe(
        &self,
        Path(user_id): Path<String>,
    ) -> impl futures_core::Stream<Item = Result<SseEvent, Infallible>> {
        self.notification_service.sse_broadcaster(&user_id).subscribe()
    }

    #[ws("/ws/{user_id}")]
    async fn ws_subscribe(&self, mut ws: WsStream, Path(user_id): Path<String>) {
        let room = self.notification_service.ws_room(&user_id);
        let mut rx = room.subscribe();
        ws.send_text(format!("Connected as user {user_id}")).await.ok();

        loop {
            tokio::select! {
                msg = ws.next() => {
                    match msg {
                        Some(Ok(Message::Close(_))) | None => break,
                        _ => {}
                    }
                }
                broadcast = rx.recv() => {
                    match broadcast {
                        Some(msg) => {
                            if ws.send(msg).await.is_err() { break; }
                        }
                        None => break,
                    }
                }
            }
        }
    }

    #[post("/send/{user_id}")]
    async fn send_to_user(
        &self,
        Path(user_id): Path<String>,
        Json(body): Json<SendNotification>,
    ) -> Json<NotificationResult> {
        self.notification_service.notify(&user_id, &body.message);
        Json(NotificationResult {
            sent_to: user_id,
            message: body.message,
        })
    }
}

#[derive(Deserialize, JsonSchema)]
struct SendNotification {
    message: String,
}

#[derive(Serialize, JsonSchema)]
struct NotificationResult {
    sent_to: String,
    message: String,
}
