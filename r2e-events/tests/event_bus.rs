use r2e_events::{
    EventBus, EventBusError, EventEnvelope, EventMetadata, HandlerResult, LocalEventBus,
    RequestOptions, DEFAULT_MAX_CONCURRENCY,
};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Serialize, Deserialize)]
struct TestEvent {
    value: usize,
}

#[derive(Serialize, Deserialize)]
struct OtherEvent;

#[derive(Serialize, Deserialize)]
struct SlowEvent;

/// Emit an event, then drain in-flight handlers — the deterministic test
/// barrier that replaced the removed `emit_and_wait`.
async fn emit_and_drain<E: Serialize + Send + Sync + 'static>(bus: &LocalEventBus, event: E) {
    bus.emit(event).await.unwrap();
    bus.wait_idle().await;
}

/// `emit_with` + drain — the metadata-carrying variant of [`emit_and_drain`].
async fn emit_with_and_drain<E: Serialize + Send + Sync + 'static>(
    bus: &LocalEventBus,
    event: E,
    metadata: EventMetadata,
) {
    bus.emit_with(event, metadata).await.unwrap();
    bus.wait_idle().await;
}

#[r2e_core::test]
async fn test_emit_and_subscribe() {
    let bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));

    let c = counter.clone();
    bus.subscribe(move |envelope: EventEnvelope<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(envelope.event.value, Ordering::SeqCst);
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    emit_and_drain(&bus, TestEvent { value: 42 }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 42);
}

#[r2e_core::test]
async fn test_multiple_subscribers() {
    let bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));

    for _ in 0..3 {
        let c = counter.clone();
        bus.subscribe(move |_: EventEnvelope<TestEvent>| {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                HandlerResult::Ack
            }
        })
        .await
        .unwrap();
    }

    emit_and_drain(&bus, TestEvent { value: 1 }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 3);
}

#[r2e_core::test]
async fn test_no_cross_type_dispatch() {
    let bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));

    let c = counter.clone();
    bus.subscribe(move |_: EventEnvelope<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    emit_and_drain(&bus, OtherEvent).await;
    assert_eq!(counter.load(Ordering::SeqCst), 0);
}

#[r2e_core::test]
async fn test_backpressure_limits_concurrency() {
    // Create a bus with max 3 concurrent handlers
    let bus = LocalEventBus::with_concurrency(3);
    let active = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));

    // Subscribe a slow handler
    let active_clone = active.clone();
    let max_clone = max_seen.clone();
    let completed_clone = completed.clone();
    bus.subscribe(move |_: EventEnvelope<SlowEvent>| {
        let active = active_clone.clone();
        let max_seen = max_clone.clone();
        let completed = completed_clone.clone();
        async move {
            // Increment active count
            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
            // Track max concurrent
            max_seen.fetch_max(current, Ordering::SeqCst);
            // Simulate work
            tokio::time::sleep(Duration::from_millis(50)).await;
            // Decrement active count
            active.fetch_sub(1, Ordering::SeqCst);
            completed.fetch_add(1, Ordering::SeqCst);
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    // Emit 10 events rapidly
    for _ in 0..10 {
        bus.emit(SlowEvent).await.unwrap();
    }

    // Wait for all handlers to complete
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Verify that we never exceeded the concurrency limit
    assert!(
        max_seen.load(Ordering::SeqCst) <= 3,
        "Max concurrent handlers ({}) exceeded limit (3)",
        max_seen.load(Ordering::SeqCst)
    );
    assert_eq!(
        completed.load(Ordering::SeqCst),
        10,
        "All events should be processed"
    );
}

#[r2e_core::test]
async fn test_unbounded_mode() {
    let bus = LocalEventBus::unbounded();
    assert!(bus.concurrency_limit().is_none());

    let counter = Arc::new(AtomicUsize::new(0));
    let c = counter.clone();
    bus.subscribe(move |_: EventEnvelope<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    emit_and_drain(&bus, TestEvent { value: 1 }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[r2e_core::test]
async fn test_with_concurrency_constructor() {
    let bus = LocalEventBus::with_concurrency(100);
    // The limit should be reported (though we can't check exact value easily)
    assert!(bus.concurrency_limit().is_some());

    let counter = Arc::new(AtomicUsize::new(0));
    let c = counter.clone();
    bus.subscribe(move |envelope: EventEnvelope<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(envelope.event.value, Ordering::SeqCst);
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    emit_and_drain(&bus, TestEvent { value: 42 }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 42);
}

// --- Phase 1: Error & Panic Isolation ---

#[r2e_core::test]
async fn test_handler_panic_does_not_crash_emit() {
    let bus = LocalEventBus::new();

    bus.subscribe(move |_: EventEnvelope<TestEvent>| async move {
        panic!("boom");
    })
    .await
    .unwrap();

    // emit spawns the handler; panic is caught by tokio::spawn
    bus.emit(TestEvent { value: 1 }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Bus should still be functional after a handler panic
    let counter = Arc::new(AtomicUsize::new(0));
    let c = counter.clone();
    bus.subscribe(move |_: EventEnvelope<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    emit_and_drain(&bus, TestEvent { value: 1 }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[r2e_core::test]
async fn test_handler_panic_does_not_crash_drain() {
    let bus = LocalEventBus::new();

    bus.subscribe(move |_: EventEnvelope<TestEvent>| async move {
        panic!("boom while draining");
    })
    .await
    .unwrap();

    // The panic is isolated to the handler task; draining must not observe it.
    emit_and_drain(&bus, TestEvent { value: 1 }).await;

    // Should reach here without panic
    let counter = Arc::new(AtomicUsize::new(0));
    let c = counter.clone();
    bus.subscribe(move |_: EventEnvelope<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    emit_and_drain(&bus, TestEvent { value: 1 }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[r2e_core::test]
async fn test_panic_releases_permit() {
    let bus = LocalEventBus::with_concurrency(1);

    // First handler panics, which should release the single permit
    bus.subscribe(move |_: EventEnvelope<TestEvent>| async move {
        panic!("release me");
    })
    .await
    .unwrap();

    bus.emit(TestEvent { value: 1 }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Second event needs the permit — if panic didn't release it, this would hang
    let counter = Arc::new(AtomicUsize::new(0));
    let c = counter.clone();
    bus.subscribe(move |_: EventEnvelope<OtherEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    emit_and_drain(&bus, OtherEvent).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[r2e_core::test]
async fn test_multiple_handlers_one_panics_others_run() {
    let bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));

    let c = counter.clone();
    bus.subscribe(move |_: EventEnvelope<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    bus.subscribe(move |_: EventEnvelope<TestEvent>| async move {
        panic!("middle handler panics");
    })
    .await
    .unwrap();

    let c = counter.clone();
    bus.subscribe(move |_: EventEnvelope<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    emit_and_drain(&bus, TestEvent { value: 1 }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[r2e_core::test]
async fn test_err_result_in_handler() {
    let bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));

    let c = counter.clone();
    bus.subscribe(move |_: EventEnvelope<TestEvent>| {
        let c = c.clone();
        async move {
            // Handler has internal error but still returns Ack
            let _: Result<(), &str> = Err("fail");
            c.fetch_add(1, Ordering::SeqCst);
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    emit_and_drain(&bus, TestEvent { value: 1 }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

// --- Phase 2: Subscription Safety ---

#[r2e_core::test]
async fn test_late_subscriber_misses_event() {
    let bus = LocalEventBus::new();
    emit_and_drain(&bus, TestEvent { value: 1 }).await;

    let counter = Arc::new(AtomicUsize::new(0));
    let c = counter.clone();
    bus.subscribe(move |_: EventEnvelope<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 0);
}

#[r2e_core::test]
async fn test_concurrent_subscribes() {
    let bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::new();
    for _ in 0..10 {
        let bus = bus.clone();
        let c = counter.clone();
        handles.push(tokio::spawn(async move {
            bus.subscribe(move |_: EventEnvelope<TestEvent>| {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    HandlerResult::Ack
                }
            })
            .await
            .unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    emit_and_drain(&bus, TestEvent { value: 1 }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 10);
}

#[r2e_core::test]
async fn test_subscribe_during_emit() {
    let bus = LocalEventBus::new();

    // Slow handler that holds processing for a while
    bus.subscribe(move |_: EventEnvelope<SlowEvent>| async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        HandlerResult::Ack
    })
    .await
    .unwrap();

    // Fire-and-forget: slow handler starts processing
    bus.emit(SlowEvent).await.unwrap();

    // Subscribe for a different event type while slow handler is running
    let counter = Arc::new(AtomicUsize::new(0));
    let c = counter.clone();
    bus.subscribe(move |_: EventEnvelope<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    emit_and_drain(&bus, TestEvent { value: 1 }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[r2e_core::test]
async fn test_subscribe_same_event_type_multiple() {
    let bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));

    for _ in 0..5 {
        let c = counter.clone();
        bus.subscribe(move |_: EventEnvelope<TestEvent>| {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                HandlerResult::Ack
            }
        })
        .await
        .unwrap();
    }

    emit_and_drain(&bus, TestEvent { value: 1 }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 5);
}

// --- Phase 3: Edge Cases & Lifecycle ---

#[r2e_core::test]
async fn test_emit_no_subscribers() {
    let bus = LocalEventBus::new();
    // Should not panic when emitting with no subscribers
    bus.emit(TestEvent { value: 1 }).await.unwrap();
}

#[r2e_core::test]
async fn test_drain_no_subscribers() {
    let bus = LocalEventBus::new();
    // Should return instantly with no subscribers, no panic
    emit_and_drain(&bus, TestEvent { value: 1 }).await;
}

#[r2e_core::test]
async fn test_default_eventbus() {
    let default_bus = LocalEventBus::default();
    let new_bus = LocalEventBus::new();
    assert_eq!(default_bus.concurrency_limit(), new_bus.concurrency_limit(),);
    assert_eq!(
        default_bus.concurrency_limit(),
        Some(DEFAULT_MAX_CONCURRENCY)
    );
}

#[r2e_core::test]
async fn test_concurrency_limit_bounded() {
    let bus = LocalEventBus::with_concurrency(5);
    assert_eq!(bus.concurrency_limit(), Some(5));
}

#[r2e_core::test]
async fn test_concurrency_limit_unbounded() {
    let bus = LocalEventBus::unbounded();
    assert_eq!(bus.concurrency_limit(), None);
}

#[r2e_core::test]
async fn test_clone_shares_state() {
    let bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));

    let c = counter.clone();
    bus.subscribe(move |_: EventEnvelope<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    // Clone the bus and emit on the clone
    let bus2 = bus.clone();
    emit_and_drain(&bus2, TestEvent { value: 1 }).await;

    // Handler registered on original should have been invoked via clone
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[r2e_core::test]
async fn test_drop_bus_with_active_handlers() {
    let bus = LocalEventBus::new();
    let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let f = flag.clone();
    bus.subscribe(move |_: EventEnvelope<SlowEvent>| {
        let f = f.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            f.store(true, std::sync::atomic::Ordering::SeqCst);
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    // Fire-and-forget, then immediately drop the bus
    bus.emit(SlowEvent).await.unwrap();
    drop(bus);

    // Spawned task should still complete despite bus being dropped
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(flag.load(std::sync::atomic::Ordering::SeqCst));
}

// --- Phase 4: Async Handler Behavior ---

#[r2e_core::test]
async fn test_handler_with_long_sleep() {
    let bus = LocalEventBus::new();
    let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let f = flag.clone();
    bus.subscribe(move |_: EventEnvelope<TestEvent>| {
        let f = f.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            f.store(true, std::sync::atomic::Ordering::SeqCst);
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    // emit() returns after spawning, not after handler completes
    bus.emit(TestEvent { value: 1 }).await.unwrap();
    assert!(!flag.load(std::sync::atomic::Ordering::SeqCst));

    // After enough time, the handler should have completed
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(flag.load(std::sync::atomic::Ordering::SeqCst));
}

#[r2e_core::test]
async fn test_drain_waits_for_slow() {
    let bus = LocalEventBus::new();
    let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let f = flag.clone();
    bus.subscribe(move |_: EventEnvelope<TestEvent>| {
        let f = f.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            f.store(true, std::sync::atomic::Ordering::SeqCst);
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    // draining should block until the handler completes
    emit_and_drain(&bus, TestEvent { value: 1 }).await;
    assert!(flag.load(std::sync::atomic::Ordering::SeqCst));
}

#[r2e_core::test]
async fn test_handler_spawns_nested_emit() {
    let bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));

    // Handler for TestEvent emits an OtherEvent
    let bus2 = bus.clone();
    bus.subscribe(move |_: EventEnvelope<TestEvent>| {
        let bus2 = bus2.clone();
        async move {
            let _ = bus2.emit(OtherEvent).await;
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    // Handler for OtherEvent increments counter
    let c = counter.clone();
    bus.subscribe(move |_: EventEnvelope<OtherEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    emit_and_drain(&bus, TestEvent { value: 1 }).await;
    // Nested emit is fire-and-forget, wait for it
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[r2e_core::test]
async fn test_handler_shared_state_mutation() {
    let bus = LocalEventBus::new();
    let data = Arc::new(tokio::sync::Mutex::new(Vec::<i32>::new()));

    for i in 1..=3 {
        let d = data.clone();
        bus.subscribe(move |_: EventEnvelope<TestEvent>| {
            let d = d.clone();
            async move {
                d.lock().await.push(i);
                HandlerResult::Ack
            }
        })
        .await
        .unwrap();
    }

    emit_and_drain(&bus, TestEvent { value: 1 }).await;

    let mut result = data.lock().await.clone();
    result.sort();
    assert_eq!(result, vec![1, 2, 3]);
}

// --- Phase 5: Stress & Performance ---

#[r2e_core::test]
async fn test_stress_many_events() {
    let bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));

    let c = counter.clone();
    bus.subscribe(move |_: EventEnvelope<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    for _ in 0..100 {
        emit_and_drain(&bus, TestEvent { value: 1 }).await;
    }
    assert_eq!(counter.load(Ordering::SeqCst), 100);
}

#[r2e_core::test]
async fn test_stress_many_subscribers() {
    let bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));

    for _ in 0..50 {
        let c = counter.clone();
        bus.subscribe(move |_: EventEnvelope<TestEvent>| {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                HandlerResult::Ack
            }
        })
        .await
        .unwrap();
    }

    emit_and_drain(&bus, TestEvent { value: 1 }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 50);
}

#[r2e_core::test]
async fn test_stress_concurrent_emit() {
    let bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));

    let c = counter.clone();
    bus.subscribe(move |_: EventEnvelope<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    let mut handles = Vec::new();
    for _ in 0..10 {
        let bus = bus.clone();
        handles.push(tokio::spawn(async move {
            for _ in 0..10 {
                emit_and_drain(&bus, TestEvent { value: 1 }).await;
            }
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    assert_eq!(counter.load(Ordering::SeqCst), 100);
}

#[r2e_core::test]
async fn test_backpressure_high_load() {
    let bus = LocalEventBus::with_concurrency(2);
    let active = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));

    let active_c = active.clone();
    let max_c = max_seen.clone();
    let completed_c = completed.clone();
    bus.subscribe(move |_: EventEnvelope<SlowEvent>| {
        let active = active_c.clone();
        let max_seen = max_c.clone();
        let completed = completed_c.clone();
        async move {
            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
            max_seen.fetch_max(current, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(30)).await;
            active.fetch_sub(1, Ordering::SeqCst);
            completed.fetch_add(1, Ordering::SeqCst);
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    for _ in 0..20 {
        bus.emit(SlowEvent).await.unwrap();
    }

    // Wait for all handlers to complete
    tokio::time::sleep(Duration::from_millis(500)).await;

    assert!(
        max_seen.load(Ordering::SeqCst) <= 2,
        "Max concurrent handlers ({}) exceeded limit (2)",
        max_seen.load(Ordering::SeqCst)
    );
    assert_eq!(completed.load(Ordering::SeqCst), 20);
}

// --- Phase 6: EventBus trait compliance ---

#[r2e_core::test]
async fn test_local_event_bus_implements_trait() {
    // Verify LocalEventBus can be used where EventBus trait is expected
    fn assert_event_bus<T: EventBus>(_bus: &T) {}
    let bus = LocalEventBus::new();
    assert_event_bus(&bus);
}

// --- Phase 7: New features (unsubscribe, metadata, shutdown, nack) ---

#[r2e_core::test]
async fn test_unsubscribe_prevents_future_dispatch() {
    let bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));

    let c = counter.clone();
    let handle = bus
        .subscribe(move |_: EventEnvelope<TestEvent>| {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                HandlerResult::Ack
            }
        })
        .await
        .unwrap();

    // First emit: handler should fire
    emit_and_drain(&bus, TestEvent { value: 1 }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    // Unsubscribe
    handle.unsubscribe();
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Second emit: handler should NOT fire
    emit_and_drain(&bus, TestEvent { value: 1 }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[r2e_core::test]
async fn test_metadata_propagated_to_handler() {
    let bus = LocalEventBus::new();
    let received_meta = Arc::new(tokio::sync::Mutex::new(None::<EventMetadata>));

    let rm = received_meta.clone();
    bus.subscribe(move |envelope: EventEnvelope<TestEvent>| {
        let rm = rm.clone();
        async move {
            *rm.lock().await = Some(envelope.metadata);
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    let meta = EventMetadata::new()
        .with_correlation_id("corr-123")
        .with_partition_key("partition-A")
        .with_header("source", "test");

    bus.emit_with(TestEvent { value: 42 }, meta).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let captured = received_meta.lock().await;
    let m = captured.as_ref().unwrap();
    assert_eq!(m.correlation_id.as_deref(), Some("corr-123"));
    assert_eq!(m.partition_key.as_deref(), Some("partition-A"));
    assert_eq!(m.headers.get("source").map(|s| s.as_str()), Some("test"));
    assert!(m.event_id > 0);
    assert!(m.timestamp > 0);
}

#[r2e_core::test]
async fn test_auto_generated_metadata_has_unique_ids() {
    let bus = LocalEventBus::new();
    let ids = Arc::new(tokio::sync::Mutex::new(Vec::<u128>::new()));

    let ids_clone = ids.clone();
    bus.subscribe(move |envelope: EventEnvelope<TestEvent>| {
        let ids = ids_clone.clone();
        async move {
            ids.lock().await.push(envelope.metadata.event_id);
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    for i in 0..5 {
        emit_and_drain(&bus, TestEvent { value: i }).await;
    }

    let collected = ids.lock().await;
    // All event_ids should be unique
    let mut deduped = collected.clone();
    deduped.sort();
    deduped.dedup();
    assert_eq!(collected.len(), deduped.len(), "event_ids should be unique");
}

#[r2e_core::test]
async fn test_shutdown_rejects_new_emits() {
    let bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));

    let c = counter.clone();
    bus.subscribe(move |_: EventEnvelope<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    // Shutdown
    bus.shutdown(Duration::from_secs(1)).await.unwrap();

    // Emit after shutdown should return Err(Shutdown)
    let result = bus.emit(TestEvent { value: 1 }).await;
    assert!(matches!(result, Err(EventBusError::Shutdown)));
    assert_eq!(counter.load(Ordering::SeqCst), 0);
}

#[r2e_core::test]
async fn test_shutdown_waits_for_in_flight() {
    let bus = LocalEventBus::new();
    let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let f = flag.clone();
    bus.subscribe(move |_: EventEnvelope<TestEvent>| {
        let f = f.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            f.store(true, std::sync::atomic::Ordering::SeqCst);
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    bus.emit(TestEvent { value: 1 }).await.unwrap();

    // Shutdown with generous timeout — should wait for handler
    bus.shutdown(Duration::from_secs(2)).await.unwrap();
    assert!(flag.load(std::sync::atomic::Ordering::SeqCst));
}

#[r2e_core::test]
async fn test_shutdown_subscribe_rejected() {
    let bus = LocalEventBus::new();
    bus.shutdown(Duration::from_secs(1)).await.unwrap();

    let result = bus
        .subscribe(move |_: EventEnvelope<TestEvent>| async move { HandlerResult::Ack })
        .await;
    assert!(matches!(result, Err(EventBusError::Shutdown)));
}

#[r2e_core::test]
async fn test_emit_with_metadata_drain() {
    let bus = LocalEventBus::new();
    let received_key = Arc::new(tokio::sync::Mutex::new(None::<String>));

    let rk = received_key.clone();
    bus.subscribe(move |envelope: EventEnvelope<TestEvent>| {
        let rk = rk.clone();
        async move {
            *rk.lock().await = envelope.metadata.partition_key;
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    let meta = EventMetadata::new().with_partition_key("my-key");
    emit_with_and_drain(&bus, TestEvent { value: 1 }, meta).await;

    let key = received_key.lock().await;
    assert_eq!(key.as_deref(), Some("my-key"));
}

#[r2e_core::test]
async fn test_handler_result_from_unit() {
    // () should convert to Ack
    let result: HandlerResult = ().into();
    assert!(matches!(result, HandlerResult::Ack));
}

#[r2e_core::test]
async fn test_handler_result_from_result() {
    let ok: HandlerResult = Ok::<(), String>(()).into();
    assert!(matches!(ok, HandlerResult::Ack));

    let err: HandlerResult = Err::<(), String>("oops".to_string()).into();
    assert!(matches!(err, HandlerResult::Nack(msg) if msg == "oops"));
}

// --- Phase 8: request / respond (point-to-point request-reply) ---

#[derive(Serialize, Deserialize)]
struct Add {
    a: i64,
    b: i64,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Sum {
    total: i64,
}

#[r2e_core::test]
async fn test_request_respond_roundtrip() {
    let bus = LocalEventBus::new();

    bus.respond(|env: EventEnvelope<Add>| async move {
        Ok::<Sum, String>(Sum {
            total: env.event.a + env.event.b,
        })
    })
    .await
    .unwrap();

    let reply: Sum = bus.request(Add { a: 2, b: 40 }).await.unwrap();
    assert_eq!(reply, Sum { total: 42 });
}

#[r2e_core::test]
async fn test_request_without_responder_is_no_responder() {
    let bus = LocalEventBus::new();
    let result: Result<Sum, _> = bus.request(Add { a: 1, b: 1 }).await;
    assert!(matches!(result, Err(EventBusError::NoResponder)));
}

#[r2e_core::test]
async fn test_responder_error_maps_to_remote() {
    let bus = LocalEventBus::new();

    bus.respond(
        |_env: EventEnvelope<Add>| async move { Err::<Sum, String>("cannot add".to_string()) },
    )
    .await
    .unwrap();

    let result: Result<Sum, _> = bus.request(Add { a: 1, b: 2 }).await;
    assert!(matches!(result, Err(EventBusError::Remote(msg)) if msg == "cannot add"));
}

#[r2e_core::test]
async fn test_request_times_out_when_responder_is_slow() {
    let bus = LocalEventBus::new();

    bus.respond(|_env: EventEnvelope<Add>| async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok::<Sum, String>(Sum { total: 0 })
    })
    .await
    .unwrap();

    let opts = RequestOptions::new().with_timeout(Duration::from_millis(20));
    let result: Result<Sum, _> = bus.request_with(Add { a: 1, b: 2 }, opts).await;
    assert!(matches!(result, Err(EventBusError::RequestTimeout)));
}

#[r2e_core::test]
async fn test_second_responder_for_same_type_is_rejected() {
    let bus = LocalEventBus::new();

    bus.respond(|_env: EventEnvelope<Add>| async move { Ok::<Sum, String>(Sum { total: 0 }) })
        .await
        .unwrap();

    let second = bus
        .respond(|_env: EventEnvelope<Add>| async move { Ok::<Sum, String>(Sum { total: 1 }) })
        .await;
    assert!(matches!(second, Err(EventBusError::Other(_))));
}

#[r2e_core::test]
async fn test_unregister_responder_allows_reregistration() {
    let bus = LocalEventBus::new();

    let handle = bus
        .respond(|_env: EventEnvelope<Add>| async move { Ok::<Sum, String>(Sum { total: 1 }) })
        .await
        .unwrap();

    handle.unregister();
    // Unregister is applied by a spawned task — give it a beat to land.
    tokio::time::sleep(Duration::from_millis(20)).await;

    bus.respond(|env: EventEnvelope<Add>| async move {
        Ok::<Sum, String>(Sum {
            total: env.event.a * env.event.b,
        })
    })
    .await
    .unwrap();

    let reply: Sum = bus.request(Add { a: 6, b: 7 }).await.unwrap();
    assert_eq!(reply, Sum { total: 42 });
}
