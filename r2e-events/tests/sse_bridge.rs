use std::pin::Pin;

use r2e_core::http::SseEvent;
use r2e_core::sse::{SseSubscription, SseTopic};
use r2e_events::sse_bridge::bridge_event_to_sse;
use r2e_events::{EventBus, LocalEventBus};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct SyncStatus {
    done: u32,
    total: u32,
}

#[derive(Serialize, Deserialize)]
struct OtherEvent;

async fn next_event(sub: &mut SseSubscription) -> Option<SseEvent> {
    tokio::time::timeout(std::time::Duration::from_millis(200), async {
        std::future::poll_fn(|cx| {
            use futures_core::Stream;
            Pin::new(&mut *sub).poll_next(cx)
        })
        .await
    })
    .await
    .ok()
    .flatten()
    .map(|r| r.unwrap())
}

#[r2e_core::test]
async fn bridged_event_reaches_sse_subscribers() {
    let bus = LocalEventBus::new();
    let topic = SseTopic::<SyncStatus>::new(16).with_event_name("sync");
    let mut sub = topic.subscribe();

    bridge_event_to_sse(&bus, topic).await.unwrap();
    bus.emit_and_wait(SyncStatus { done: 10, total: 42 })
        .await
        .unwrap();

    let event = next_event(&mut sub).await.expect("SSE stream should yield the bridged event");
    let debug = format!("{event:?}");
    assert!(debug.contains("sync"), "SSE event name should be the topic's: {debug}");
    assert!(
        debug.contains(r#"\"done\":10"#) || debug.contains(r#""done":10"#),
        "SSE data should be the JSON-serialized event: {debug}"
    );
}

#[r2e_core::test]
async fn bridge_ignores_other_event_types() {
    let bus = LocalEventBus::new();
    let topic = SseTopic::<SyncStatus>::new(16);
    let mut sub = topic.subscribe();

    bridge_event_to_sse(&bus, topic).await.unwrap();
    bus.emit_and_wait(OtherEvent).await.unwrap();

    assert!(
        next_event(&mut sub).await.is_none(),
        "an event of another type must not be forwarded"
    );
}

#[r2e_core::test]
async fn bridge_with_no_sse_subscribers_still_acks() {
    let bus = LocalEventBus::new();
    let topic = SseTopic::<SyncStatus>::new(16);

    bridge_event_to_sse(&bus, topic).await.unwrap();
    // No SSE clients connected — emit must still succeed (publish is Ok(0)).
    bus.emit_and_wait(SyncStatus { done: 0, total: 0 })
        .await
        .unwrap();
}

#[r2e_core::test]
async fn unsubscribing_the_bridge_stops_forwarding() {
    let bus = LocalEventBus::new();
    let topic = SseTopic::<SyncStatus>::new(16);
    let mut sub = topic.subscribe();

    let handle = bridge_event_to_sse(&bus, topic).await.unwrap();
    handle.unsubscribe();
    // Unsubscription is applied by a spawned task — give it a beat to land.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    bus.emit_and_wait(SyncStatus { done: 1, total: 1 })
        .await
        .unwrap();

    assert!(
        next_event(&mut sub).await.is_none(),
        "no events should be forwarded after unsubscribe"
    );
}
