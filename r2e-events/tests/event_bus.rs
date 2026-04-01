use r2e_events::{EventBus, EventBusError, EventEnvelope, EventMetadata, HandlerResult, LocalEventBus, DEFAULT_MAX_CONCURRENCY};
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

    bus.emit_and_wait(TestEvent { value: 42 }).await.unwrap();
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

    bus.emit_and_wait(TestEvent { value: 1 }).await.unwrap();
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

    bus.emit_and_wait(OtherEvent).await.unwrap();
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
    assert_eq!(completed.load(Ordering::SeqCst), 10, "All events should be processed");
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

    bus.emit_and_wait(TestEvent { value: 1 }).await.unwrap();
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

    bus.emit_and_wait(TestEvent { value: 42 }).await.unwrap();
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

    bus.emit_and_wait(TestEvent { value: 1 }).await.unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[r2e_core::test]
async fn test_handler_panic_does_not_crash_emit_and_wait() {
    let bus = LocalEventBus::new();

    bus.subscribe(move |_: EventEnvelope<TestEvent>| async move {
        panic!("boom in emit_and_wait");
    })
    .await
    .unwrap();

    // emit_and_wait does `let _ = task.await` which swallows JoinError
    bus.emit_and_wait(TestEvent { value: 1 }).await.unwrap();

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

    bus.emit_and_wait(TestEvent { value: 1 }).await.unwrap();
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

    bus.emit_and_wait(OtherEvent).await.unwrap();
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

    bus.emit_and_wait(TestEvent { value: 1 }).await.unwrap();
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

    bus.emit_and_wait(TestEvent { value: 1 }).await.unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

// --- Phase 2: Subscription Safety ---

#[r2e_core::test]
async fn test_late_subscriber_misses_event() {
    let bus = LocalEventBus::new();
    bus.emit_and_wait(TestEvent { value: 1 }).await.unwrap();

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

    bus.emit_and_wait(TestEvent { value: 1 }).await.unwrap();
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

    bus.emit_and_wait(TestEvent { value: 1 }).await.unwrap();
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

    bus.emit_and_wait(TestEvent { value: 1 }).await.unwrap();
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
async fn test_emit_and_wait_no_subscribers() {
    let bus = LocalEventBus::new();
    // Should return instantly with no subscribers, no panic
    bus.emit_and_wait(TestEvent { value: 1 }).await.unwrap();
}

#[r2e_core::test]
async fn test_default_eventbus() {
    let default_bus = LocalEventBus::default();
    let new_bus = LocalEventBus::new();
    assert_eq!(
        default_bus.concurrency_limit(),
        new_bus.concurrency_limit(),
    );
    assert_eq!(default_bus.concurrency_limit(), Some(DEFAULT_MAX_CONCURRENCY));
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
    bus2.emit_and_wait(TestEvent { value: 1 }).await.unwrap();

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
async fn test_emit_and_wait_waits_for_slow() {
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

    // emit_and_wait should block until handler completes
    bus.emit_and_wait(TestEvent { value: 1 }).await.unwrap();
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

    bus.emit_and_wait(TestEvent { value: 1 }).await.unwrap();
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

    bus.emit_and_wait(TestEvent { value: 1 }).await.unwrap();

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
        bus.emit_and_wait(TestEvent { value: 1 }).await.unwrap();
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

    bus.emit_and_wait(TestEvent { value: 1 }).await.unwrap();
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
                bus.emit_and_wait(TestEvent { value: 1 }).await.unwrap();
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
    let handle = bus.subscribe(move |_: EventEnvelope<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            HandlerResult::Ack
        }
    })
    .await
    .unwrap();

    // First emit: handler should fire
    bus.emit_and_wait(TestEvent { value: 1 }).await.unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    // Unsubscribe
    handle.unsubscribe();
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Second emit: handler should NOT fire
    bus.emit_and_wait(TestEvent { value: 1 }).await.unwrap();
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
    let ids = Arc::new(tokio::sync::Mutex::new(Vec::<u64>::new()));

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
        bus.emit_and_wait(TestEvent { value: i }).await.unwrap();
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

    let result = bus.subscribe(move |_: EventEnvelope<TestEvent>| async move {
        HandlerResult::Ack
    }).await;
    assert!(matches!(result, Err(EventBusError::Shutdown)));
}

#[r2e_core::test]
async fn test_emit_and_wait_with_metadata() {
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
    bus.emit_and_wait_with(TestEvent { value: 1 }, meta).await.unwrap();

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
