use r2e::prelude::*;
use r2e::ws::{WsRooms, WsStream};

use crate::models::{MessageSentEvent, WsIncoming, WsOutgoing};
use crate::state::AppState;

#[derive(Controller)]
#[controller(path = "/chat", state = AppState)]
pub struct ChatController {
    #[inject]
    ws_rooms: WsRooms,
    #[inject]
    event_bus: LocalEventBus,
}

#[routes]
impl ChatController {
    /// WebSocket endpoint: /chat/{room}?username=Alice
    #[ws("/{room}")]
    async fn join_room(
        &self,
        Path(room): Path<String>,
        Query(params): Query<std::collections::HashMap<String, String>>,
        mut ws: WsStream,
    ) {
        let username = params
            .get("username")
            .cloned()
            .unwrap_or_else(|| "Anonymous".to_string());

        let broadcaster = self.ws_rooms.room(&room);
        let mut rx = broadcaster.subscribe();
        let client_id = rx.client_id();

        // Announce join to all clients in the room
        let _ = broadcaster.send_json(&WsOutgoing::Join {
            username: username.clone(),
            room: room.clone(),
        });

        let event_bus = self.event_bus.clone();

        loop {
            tokio::select! {
                // Forward broadcast messages from other clients to this WS
                msg = rx.recv() => {
                    match msg {
                        Some(msg) => {
                            if ws.send(msg).await.is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
                // Read messages from this client's WS
                incoming = ws.next_json::<WsIncoming>() => {
                    match incoming {
                        Some(Ok(WsIncoming::Message { text })) => {
                            let outgoing = WsOutgoing::Message {
                                username: username.clone(),
                                text: text.clone(),
                                room: room.clone(),
                            };

                            // Broadcast to other clients (excludes sender)
                            let _ = broadcaster.send_json_from(client_id, &outgoing);
                            // Echo back to sender
                            let _ = ws.send_json(&outgoing).await;

                            // Emit event for persistence (fire-and-forget)
                            event_bus.emit(MessageSentEvent {
                                room: room.clone(),
                                username: username.clone(),
                                text,
                            }).await;
                        }
                        Some(Err(_)) => continue,
                        None => break,
                    }
                }
            }
        }

        // Announce leave
        let _ = broadcaster.send_json(&WsOutgoing::Leave {
            username,
            room,
        });
    }
}
