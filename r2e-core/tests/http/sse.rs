use std::pin::Pin;

use r2e_core::http::SseEvent;
use r2e_core::web::sse::{LagPolicy, SseBroadcaster, SseRooms, SseSubscription};

async fn next_event(sub: &mut SseSubscription) -> Option<SseEvent> {
    tokio::time::timeout(std::time::Duration::from_millis(100), async {
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

/// Single-poll read — returns `None` immediately if the stream is `Pending`.
/// Use for negative assertions ("should have no more events") to avoid
/// burning wall-clock on a guaranteed no-op wait.
fn poll_once(sub: &mut SseSubscription) -> Option<SseEvent> {
    use futures_core::Stream;
    use std::task::{Context, Poll};
    let waker = std::task::Waker::noop();
    let mut cx = Context::from_waker(waker);
    match Pin::new(sub).poll_next(&mut cx) {
        Poll::Ready(Some(Ok(e))) => Some(e),
        _ => None,
    }
}

#[r2e_core::test]
async fn sse_broadcaster_send_recv() {
    let broadcaster = SseBroadcaster::new(16);
    let mut sub = broadcaster.subscribe();
    broadcaster.send("hello").unwrap();
    let event = next_event(&mut sub).await.expect("should receive event");
    // SseEvent doesn't expose fields directly, so check via Debug repr
    let debug = format!("{event:?}");
    assert!(
        debug.contains("hello"),
        "event debug should contain data: {debug}"
    );
}

#[r2e_core::test]
async fn sse_broadcaster_typed_event() {
    let broadcaster = SseBroadcaster::new(16);
    let mut sub = broadcaster.subscribe();
    broadcaster.send_event("msg", "payload").unwrap();
    let event = next_event(&mut sub).await.expect("should receive event");
    let debug = format!("{event:?}");
    assert!(
        debug.contains("msg"),
        "event debug should contain event type: {debug}"
    );
    assert!(
        debug.contains("payload"),
        "event debug should contain data: {debug}"
    );
}

#[r2e_core::test]
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

#[r2e_core::test]
async fn sse_broadcaster_reports_capacity_and_subscribers() {
    let broadcaster = SseBroadcaster::new(32);
    assert_eq!(broadcaster.capacity(), 32);
    assert_eq!(broadcaster.subscriber_count(), 0);

    let _sub1 = broadcaster.subscribe();
    let _sub2 = broadcaster.subscribe();
    assert_eq!(broadcaster.subscriber_count(), 2);
}

#[r2e_core::test]
async fn sse_broadcaster_send_errors_without_subscribers() {
    let broadcaster = SseBroadcaster::new(16);
    // No subscribers yet — the tokio broadcast channel reports SendError.
    assert!(broadcaster.send("lost").is_err());
}

#[r2e_core::test]
async fn sse_silent_subscription_drops_lagged_messages() {
    // Capacity 2 so the 3rd send pushes subscriber past its window.
    let broadcaster = SseBroadcaster::new(2);
    let mut sub = broadcaster.subscribe();

    // Fill beyond capacity without polling — the subscriber should lag.
    broadcaster.send("one").unwrap();
    broadcaster.send("two").unwrap();
    broadcaster.send("three").unwrap();
    broadcaster.send("four").unwrap();

    // Silent mode: oldest dropped, newest two delivered. No synthetic event.
    let e1 = next_event(&mut sub).await.expect("sub should recover");
    let d1 = format!("{e1:?}");
    assert!(
        d1.contains("three") || d1.contains("four"),
        "expected a non-dropped message, got: {d1}"
    );
    // Drain any remaining message.
    let _ = next_event(&mut sub).await;
    // Stream should now be pending, not terminated.
    assert!(broadcaster.subscriber_count() >= 1);
}

#[r2e_core::test]
async fn sse_lag_event_surfaces_synthetic_event() {
    let broadcaster = SseBroadcaster::new(2);
    let mut sub = broadcaster.subscribe_lagged("lagged");

    // Overflow the subscriber's window.
    broadcaster.send("one").unwrap();
    broadcaster.send("two").unwrap();
    broadcaster.send("three").unwrap();
    broadcaster.send("four").unwrap();

    // First event out should be the synthetic "lagged" event.
    let event = next_event(&mut sub)
        .await
        .expect("should receive lag event");
    let debug = format!("{event:?}");
    assert!(
        debug.contains("lagged"),
        "expected synthetic lag event, got: {debug}"
    );

    // Subsequent events should be the still-buffered real messages.
    let real = next_event(&mut sub)
        .await
        .expect("should receive real event");
    let real_debug = format!("{real:?}");
    assert!(
        real_debug.contains("three") || real_debug.contains("four"),
        "expected a recovered message, got: {real_debug}"
    );
}

// ── SseRooms ────────────────────────────────────────────────────────────

#[r2e_core::test]
async fn sse_rooms_lazy_insert_and_broadcast_per_key() {
    let rooms: SseRooms<String> = SseRooms::new(16);
    assert_eq!(rooms.room_count(), 0);
    assert!(rooms.is_empty());

    let mut sub_a = rooms.subscribe("run-a".to_string());
    assert_eq!(rooms.room_count(), 1);

    let mut sub_b = rooms.subscribe("run-b".to_string());
    assert_eq!(rooms.room_count(), 2);

    rooms.room("run-a".to_string()).send("for-a").unwrap();
    rooms.room("run-b".to_string()).send("for-b").unwrap();

    let ea = next_event(&mut sub_a).await.expect("sub_a should receive");
    let eb = next_event(&mut sub_b).await.expect("sub_b should receive");

    let da = format!("{ea:?}");
    let db = format!("{eb:?}");
    assert!(da.contains("for-a"), "sub_a should only see its room: {da}");
    assert!(db.contains("for-b"), "sub_b should only see its room: {db}");

    // Cross-talk check: sub_a must not see for-b. Single poll — no wall-clock wait.
    let lingering = poll_once(&mut sub_a);
    assert!(
        lingering.is_none(),
        "sub_a should not have cross-room traffic: {lingering:?}"
    );
}

#[r2e_core::test]
async fn sse_rooms_remove_drops_broadcaster() {
    let rooms: SseRooms<u64> = SseRooms::new(8);
    let _ = rooms.room(42);
    assert_eq!(rooms.room_count(), 1);
    rooms.remove(&42);
    assert_eq!(rooms.room_count(), 0);
}

#[r2e_core::test]
async fn sse_rooms_reap_empty_drops_subscriberless_rooms() {
    let rooms: SseRooms<String> = SseRooms::new(16);
    // Room with a live subscriber (kept across the reap).
    let _sub = rooms.subscribe("kept".to_string());
    // Room with no subscriber (will be reaped).
    let _abandoned = rooms.room("abandoned".to_string());

    assert_eq!(rooms.room_count(), 2);
    let reaped = rooms.reap_empty();
    assert_eq!(reaped, 1);
    assert_eq!(rooms.room_count(), 1);
}

#[r2e_core::test]
async fn sse_subscribe_with_policy_delegates() {
    // subscribe_with(Silent) should behave like subscribe().
    let broadcaster = SseBroadcaster::new(2);
    let mut sub = broadcaster.subscribe_with(LagPolicy::Silent);
    broadcaster.send("one").unwrap();
    broadcaster.send("two").unwrap();
    broadcaster.send("three").unwrap();
    // No synthetic event — oldest dropped silently.
    let event = next_event(&mut sub).await.expect("should receive event");
    let debug = format!("{event:?}");
    assert!(
        debug.contains("two") || debug.contains("three"),
        "expected non-dropped message, got: {debug}"
    );

    // subscribe_with(Synthetic(name)) should behave like subscribe_lagged(name).
    let broadcaster = SseBroadcaster::new(2);
    let mut sub = broadcaster.subscribe_with(LagPolicy::Synthetic("dropped".to_string()));
    broadcaster.send("one").unwrap();
    broadcaster.send("two").unwrap();
    broadcaster.send("three").unwrap();
    let event = next_event(&mut sub)
        .await
        .expect("should receive lag event");
    let debug = format!("{event:?}");
    assert!(
        debug.contains("dropped"),
        "expected synthetic event, got: {debug}"
    );
}

#[r2e_core::test]
async fn sse_rooms_typed_key() {
    // Demonstrate that SseRooms<K> supports typed keys (not just String).
    #[derive(Clone, Debug, Eq, Hash, PartialEq)]
    struct RunId(u64);

    let rooms: SseRooms<RunId> = SseRooms::new(8);
    let mut sub = rooms.subscribe(RunId(1));
    rooms.room(RunId(1)).send("typed").unwrap();
    let event = next_event(&mut sub).await.expect("typed room should work");
    assert!(format!("{event:?}").contains("typed"));
}

// ── SseTopic ─────────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
struct SyncStatus {
    done: u32,
    total: u32,
}

#[r2e_core::test]
async fn sse_topic_default_event_name_is_short_type_name() {
    let topic = r2e_core::web::sse::SseTopic::<SyncStatus>::new(16);
    assert_eq!(topic.event_name(), "SyncStatus");
}

#[r2e_core::test]
async fn sse_topic_generic_event_name_strips_generics() {
    let topic = r2e_core::web::sse::SseTopic::<Vec<SyncStatus>>::new(16);
    assert_eq!(topic.event_name(), "Vec");
}

#[r2e_core::test]
async fn sse_topic_with_event_name_overrides_default() {
    let topic = r2e_core::web::sse::SseTopic::<SyncStatus>::new(16).with_event_name("sync");
    assert_eq!(topic.event_name(), "sync");
}

#[r2e_core::test]
async fn sse_topic_publish_roundtrip() {
    let topic = r2e_core::web::sse::SseTopic::<SyncStatus>::new(16).with_event_name("sync");
    let mut sub = topic.subscribe();
    let received = topic
        .publish(&SyncStatus {
            done: 10,
            total: 42,
        })
        .unwrap();
    assert_eq!(received, 1);
    let event = next_event(&mut sub).await.expect("should receive event");
    let debug = format!("{event:?}");
    assert!(
        debug.contains("sync"),
        "event name should be on the wire: {debug}"
    );
    assert!(
        debug.contains(r#"\"done\":10"#) || debug.contains(r#""done":10"#),
        "JSON payload should be on the wire: {debug}"
    );
}

#[r2e_core::test]
async fn sse_topic_publish_without_subscribers_is_ok_zero() {
    let topic = r2e_core::web::sse::SseTopic::<SyncStatus>::new(16);
    let received = topic.publish(&SyncStatus { done: 0, total: 0 }).unwrap();
    assert_eq!(received, 0);
}

#[r2e_core::test]
async fn sse_topic_clone_shares_channel() {
    let topic = r2e_core::web::sse::SseTopic::<SyncStatus>::new(16);
    let clone = topic.clone();
    let mut sub = clone.subscribe();
    topic.publish(&SyncStatus { done: 1, total: 2 }).unwrap();
    assert!(
        next_event(&mut sub).await.is_some(),
        "clone should share the broadcast channel"
    );
}

#[r2e_core::test]
async fn sse_topic_custom_serializer() {
    let topic = r2e_core::web::sse::SseTopic::<SyncStatus>::new(16)
        .with_event_name("sync")
        .with_serializer(|s| Ok(format!("{}/{}", s.done, s.total)));
    let mut sub = topic.subscribe();
    topic
        .publish(&SyncStatus {
            done: 10,
            total: 42,
        })
        .unwrap();
    let event = next_event(&mut sub).await.expect("should receive event");
    let debug = format!("{event:?}");
    assert!(
        debug.contains("10/42"),
        "custom format should be on the wire: {debug}"
    );
}

#[r2e_core::test]
async fn sse_topic_serializer_error_is_returned() {
    let topic = r2e_core::web::sse::SseTopic::<SyncStatus>::new(16)
        .with_serializer(|_| Err("encoding broke".into()));
    let mut sub = topic.subscribe();
    let err = topic
        .publish(&SyncStatus { done: 1, total: 1 })
        .unwrap_err();
    assert!(err.to_string().contains("encoding broke"));
    assert!(
        poll_once(&mut sub).is_none(),
        "nothing should have been broadcast"
    );
}

// ── Shutdown termination ─────────────────────────────────────────────────

mod shutdown {
    use std::convert::Infallible;
    use std::time::Duration;

    use futures_core::Stream;
    use http_body_util::BodyExt;
    use r2e_core::http::{Body, Request, SseEvent};
    use r2e_core::prelude::*;
    use r2e_core::rt::{CancelToken, ShutdownToken};
    use r2e_core::web::sse::{until_shutdown, SseBroadcaster};
    use tower::ServiceExt;

    /// One chunk of the streamed response body, or `None` if the body ended.
    async fn next_chunk(body: &mut Body) -> Option<String> {
        loop {
            let frame = tokio::time::timeout(Duration::from_millis(500), body.frame())
                .await
                .expect("body stalled")?;
            let frame = frame.expect("body error");
            if let Ok(data) = frame.into_data() {
                return Some(String::from_utf8_lossy(&data).to_string());
            }
        }
    }

    #[controller(path = "/live")]
    struct LiveController {
        #[inject]
        bus: SseBroadcaster,
    }

    #[routes]
    impl LiveController {
        /// A stream that never ends on its own — only the shutdown wrapper can
        /// terminate it.
        #[sse("/events", keep_alive = false)]
        async fn events(&self) -> impl Stream<Item = Result<SseEvent, Infallible>> {
            self.bus.subscribe()
        }
    }

    #[r2e_core::test]
    async fn sse_route_ends_when_the_app_shuts_down() {
        let shutdown = CancelToken::new();
        let bus = SseBroadcaster::new(16);
        let router = AppBuilder::new()
            .override_bean(ShutdownToken::from_token(shutdown.clone()))
            .provide(bus.clone())
            .build_state()
            .await
            .register_controller::<LiveController>()
            .build();

        let req = Request::builder()
            .uri("/live/events")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), r2e_core::http::StatusCode::OK);
        let mut body = resp.into_body();

        bus.send("one").unwrap();
        let chunk = next_chunk(&mut body).await.expect("first event");
        assert!(chunk.contains("one"), "unexpected chunk: {chunk:?}");

        // Nothing ends this stream but the shutdown signal.
        shutdown.cancel();
        assert!(
            next_chunk(&mut body).await.is_none(),
            "the SSE body must end when the app shuts down"
        );
    }

    #[r2e_core::test]
    async fn until_shutdown_forwards_items_until_cancelled() {
        use futures_util::StreamExt as _;

        let token = CancelToken::new();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<u32>();
        let inner = r2e_core::rt::stream::wrappers::UnboundedReceiverStream::new(rx);
        let mut stream = Box::pin(until_shutdown(
            inner,
            Some(ShutdownToken::from_token(token.clone())),
        ));

        tx.send(1).unwrap();
        assert_eq!(stream.next().await, Some(1));
        token.cancel();
        assert_eq!(
            stream.next().await,
            None,
            "cancellation must end the stream even though the sender is alive"
        );
        drop(tx);
    }

    #[r2e_core::test]
    async fn until_shutdown_is_transparent_without_a_token() {
        use futures_util::StreamExt as _;

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<u32>();
        let inner = r2e_core::rt::stream::wrappers::UnboundedReceiverStream::new(rx);
        let mut stream = Box::pin(until_shutdown(inner, None));

        tx.send(7).unwrap();
        assert_eq!(stream.next().await, Some(7));
        drop(tx);
        assert_eq!(stream.next().await, None, "the inner stream still ends it");
    }
}
