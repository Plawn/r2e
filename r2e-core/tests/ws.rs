#![cfg(feature = "ws")]

use axum::extract::ws::Message;
use r2e_core::ws::{WsBroadcaster, WsRooms};

#[tokio::test]
async fn broadcaster_send_recv() {
    let broadcaster = WsBroadcaster::new(16);
    let mut rx = broadcaster.subscribe();
    broadcaster.send_text("hello");
    let msg = rx.recv().await.unwrap();
    match msg {
        Message::Text(t) => assert_eq!(t.to_string(), "hello"),
        other => panic!("expected Text, got {other:?}"),
    }
}

#[tokio::test]
async fn broadcaster_excludes_sender() {
    let broadcaster = WsBroadcaster::new(16);
    let mut rx = broadcaster.subscribe();
    let sender_id = rx.client_id();

    // Send from the same client id — should be skipped
    broadcaster.send_text_from(sender_id, "self-msg");

    // Send from a different client id — should be received
    broadcaster.send_text_from(sender_id + 999, "other-msg");

    let msg = rx.recv().await.unwrap();
    match msg {
        Message::Text(t) => assert_eq!(t.to_string(), "other-msg"),
        other => panic!("expected Text, got {other:?}"),
    }
}

#[tokio::test]
async fn rooms_get_or_create() {
    let rooms = WsRooms::new(16);
    let _b1 = rooms.room("chat");
    let _b2 = rooms.room("chat");
    // Both should work (same room reused internally)
    assert_eq!(rooms.room_count(), 1);
}

#[tokio::test]
async fn rooms_remove() {
    let rooms = WsRooms::new(16);
    let _b = rooms.room("chat");
    assert_eq!(rooms.room_count(), 1);
    rooms.remove("chat");
    assert_eq!(rooms.room_count(), 0);
    // Creating again should work
    let _b2 = rooms.room("chat");
    assert_eq!(rooms.room_count(), 1);
}
