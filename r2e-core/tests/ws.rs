#![cfg(feature = "ws")]

use r2e_core::http::ws::Message;
use r2e_core::ws::{WsBroadcaster, WsRooms};

#[r2e_core::test]
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

#[r2e_core::test]
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

#[r2e_core::test]
async fn rooms_get_or_create() {
    let rooms: WsRooms = WsRooms::new(16);
    let _b1 = rooms.room("chat".to_string());
    let _b2 = rooms.room("chat".to_string());
    // Both should work (same room reused internally)
    assert_eq!(rooms.room_count(), 1);
}

#[r2e_core::test]
async fn rooms_remove() {
    let rooms: WsRooms = WsRooms::new(16);
    let _b = rooms.room("chat".to_string());
    assert_eq!(rooms.room_count(), 1);
    rooms.remove("chat");
    assert_eq!(rooms.room_count(), 0);
    // Creating again should work
    let _b2 = rooms.room("chat".to_string());
    assert_eq!(rooms.room_count(), 1);
}

#[r2e_core::test]
async fn rooms_reap_empty_drops_subscriberless_rooms() {
    let rooms: WsRooms = WsRooms::new(16);
    // Room with a live subscriber (kept alive across the reap).
    let kept_broadcaster = rooms.room("kept".to_string());
    let _rx = kept_broadcaster.subscribe();
    // Room with no subscriber (will be reaped).
    let _abandoned = rooms.room("abandoned".to_string());

    assert_eq!(rooms.room_count(), 2);
    let reaped = rooms.reap_empty();
    assert_eq!(reaped, 1);
    assert_eq!(rooms.room_count(), 1);
}

#[r2e_core::test]
async fn rooms_typed_key() {
    #[derive(Clone, Debug, Eq, Hash, PartialEq)]
    struct ChatRoomId(u64);

    let rooms: WsRooms<ChatRoomId> = WsRooms::new(8);
    let _b = rooms.room(ChatRoomId(1));
    assert_eq!(rooms.room_count(), 1);
    rooms.remove(&ChatRoomId(1));
    assert_eq!(rooms.room_count(), 0);
}
