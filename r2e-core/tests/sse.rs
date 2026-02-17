use std::pin::Pin;

use axum::response::sse::Event as SseEvent;
use r2e_core::sse::{SseBroadcaster, SseSubscription};

async fn next_event(sub: &mut SseSubscription) -> Option<SseEvent> {
    tokio::time::timeout(std::time::Duration::from_millis(100), async {
        std::future::poll_fn(|cx| {
            use futures_core::Stream;
            Pin::new(&mut *sub).poll_next(cx)
        }).await
    })
    .await
    .ok()
    .flatten()
    .map(|r| r.unwrap())
}

#[tokio::test]
async fn sse_broadcaster_send_recv() {
    let broadcaster = SseBroadcaster::new(16);
    let mut sub = broadcaster.subscribe();
    broadcaster.send("hello").unwrap();
    let event = next_event(&mut sub).await.expect("should receive event");
    // SseEvent doesn't expose fields directly, so check via Debug repr
    let debug = format!("{event:?}");
    assert!(debug.contains("hello"), "event debug should contain data: {debug}");
}

#[tokio::test]
async fn sse_broadcaster_typed_event() {
    let broadcaster = SseBroadcaster::new(16);
    let mut sub = broadcaster.subscribe();
    broadcaster.send_event("msg", "payload").unwrap();
    let event = next_event(&mut sub).await.expect("should receive event");
    let debug = format!("{event:?}");
    assert!(debug.contains("msg"), "event debug should contain event type: {debug}");
    assert!(debug.contains("payload"), "event debug should contain data: {debug}");
}

#[tokio::test]
async fn sse_multiple_subscribers() {
    let broadcaster = SseBroadcaster::new(16);
    let mut sub1 = broadcaster.subscribe();
    let mut sub2 = broadcaster.subscribe();
    broadcaster.send("shared").unwrap();
    let e1 = next_event(&mut sub1).await.expect("sub1 should receive");
    let e2 = next_event(&mut sub2).await.expect("sub2 should receive");
    let d1 = format!("{e1:?}");
    let d2 = format!("{e2:?}");
    assert!(d1.contains("shared"));
    assert!(d2.contains("shared"));
}
