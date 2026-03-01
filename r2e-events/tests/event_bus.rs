use r2e_events::{EventBus, LocalEventBus, DEFAULT_MAX_CONCURRENCY};
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

#[tokio::test]
async fn test_emit_and_subscribe() {
    let bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));

    let c = counter.clone();
    bus.subscribe(move |event: Arc<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(event.value, Ordering::SeqCst);
        }
    })
    .await;

    bus.emit_and_wait(TestEvent { value: 42 }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 42);
}

#[tokio::test]
async fn test_multiple_subscribers() {
    let bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));

    for _ in 0..3 {
        let c = counter.clone();
        bus.subscribe(move |_: Arc<TestEvent>| {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
            }
        })
        .await;
    }

    bus.emit_and_wait(TestEvent { value: 1 }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn test_no_cross_type_dispatch() {
    let bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));

    let c = counter.clone();
    bus.subscribe(move |_: Arc<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
        }
    })
    .await;

    bus.emit_and_wait(OtherEvent).await;
    assert_eq!(counter.load(Ordering::SeqCst), 0);
}

#[tokio::test]
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
    bus.subscribe(move |_: Arc<SlowEvent>| {
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
        }
    })
    .await;

    // Emit 10 events rapidly
    for _ in 0..10 {
        bus.emit(SlowEvent).await;
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

#[tokio::test]
async fn test_unbounded_mode() {
    let bus = LocalEventBus::unbounded();
    assert!(bus.concurrency_limit().is_none());

    let counter = Arc::new(AtomicUsize::new(0));
    let c = counter.clone();
    bus.subscribe(move |_: Arc<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
        }
    })
    .await;

    bus.emit_and_wait(TestEvent { value: 1 }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_with_concurrency_constructor() {
    let bus = LocalEventBus::with_concurrency(100);
    // The limit should be reported (though we can't check exact value easily)
    assert!(bus.concurrency_limit().is_some());

    let counter = Arc::new(AtomicUsize::new(0));
    let c = counter.clone();
    bus.subscribe(move |event: Arc<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(event.value, Ordering::SeqCst);
        }
    })
    .await;

    bus.emit_and_wait(TestEvent { value: 42 }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 42);
}

// --- Phase 1: Error & Panic Isolation ---

#[tokio::test]
async fn test_handler_panic_does_not_crash_emit() {
    let bus = LocalEventBus::new();

    bus.subscribe(move |_: Arc<TestEvent>| async move {
        panic!("boom");
    })
    .await;

    // emit spawns the handler; panic is caught by tokio::spawn
    bus.emit(TestEvent { value: 1 }).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Bus should still be functional after a handler panic
    let counter = Arc::new(AtomicUsize::new(0));
    let c = counter.clone();
    bus.subscribe(move |_: Arc<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
        }
    })
    .await;

    bus.emit_and_wait(TestEvent { value: 1 }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_handler_panic_does_not_crash_emit_and_wait() {
    let bus = LocalEventBus::new();

    bus.subscribe(move |_: Arc<TestEvent>| async move {
        panic!("boom in emit_and_wait");
    })
    .await;

    // emit_and_wait does `let _ = task.await` which swallows JoinError
    bus.emit_and_wait(TestEvent { value: 1 }).await;

    // Should reach here without panic
    let counter = Arc::new(AtomicUsize::new(0));
    let c = counter.clone();
    bus.subscribe(move |_: Arc<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
        }
    })
    .await;

    bus.emit_and_wait(TestEvent { value: 1 }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_panic_releases_permit() {
    let bus = LocalEventBus::with_concurrency(1);

    // First handler panics, which should release the single permit
    bus.subscribe(move |_: Arc<TestEvent>| async move {
        panic!("release me");
    })
    .await;

    bus.emit(TestEvent { value: 1 }).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Second event needs the permit — if panic didn't release it, this would hang
    let counter = Arc::new(AtomicUsize::new(0));
    let c = counter.clone();
    bus.subscribe(move |_: Arc<OtherEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
        }
    })
    .await;

    bus.emit_and_wait(OtherEvent).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_multiple_handlers_one_panics_others_run() {
    let bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));

    let c = counter.clone();
    bus.subscribe(move |_: Arc<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
        }
    })
    .await;

    bus.subscribe(move |_: Arc<TestEvent>| async move {
        panic!("middle handler panics");
    })
    .await;

    let c = counter.clone();
    bus.subscribe(move |_: Arc<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
        }
    })
    .await;

    bus.emit_and_wait(TestEvent { value: 1 }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_err_result_in_handler() {
    let bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));

    let c = counter.clone();
    bus.subscribe(move |_: Arc<TestEvent>| {
        let c = c.clone();
        async move {
            // Handler has internal error but still returns ()
            let _: Result<(), &str> = Err("fail");
            c.fetch_add(1, Ordering::SeqCst);
        }
    })
    .await;

    bus.emit_and_wait(TestEvent { value: 1 }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

// --- Phase 2: Subscription Safety ---

#[tokio::test]
async fn test_late_subscriber_misses_event() {
    let bus = LocalEventBus::new();
    bus.emit_and_wait(TestEvent { value: 1 }).await;

    let counter = Arc::new(AtomicUsize::new(0));
    let c = counter.clone();
    bus.subscribe(move |_: Arc<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
        }
    })
    .await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn test_concurrent_subscribes() {
    let bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::new();
    for _ in 0..10 {
        let bus = bus.clone();
        let c = counter.clone();
        handles.push(tokio::spawn(async move {
            bus.subscribe(move |_: Arc<TestEvent>| {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                }
            })
            .await;
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    bus.emit_and_wait(TestEvent { value: 1 }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 10);
}

#[tokio::test]
async fn test_subscribe_during_emit() {
    let bus = LocalEventBus::new();

    // Slow handler that holds processing for a while
    bus.subscribe(move |_: Arc<SlowEvent>| async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
    })
    .await;

    // Fire-and-forget: slow handler starts processing
    bus.emit(SlowEvent).await;

    // Subscribe for a different event type while slow handler is running
    let counter = Arc::new(AtomicUsize::new(0));
    let c = counter.clone();
    bus.subscribe(move |_: Arc<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
        }
    })
    .await;

    bus.emit_and_wait(TestEvent { value: 1 }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_subscribe_same_event_type_multiple() {
    let bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));

    for _ in 0..5 {
        let c = counter.clone();
        bus.subscribe(move |_: Arc<TestEvent>| {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
            }
        })
        .await;
    }

    bus.emit_and_wait(TestEvent { value: 1 }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 5);
}

// --- Phase 3: Edge Cases & Lifecycle ---

#[tokio::test]
async fn test_emit_no_subscribers() {
    let bus = LocalEventBus::new();
    // Should not panic when emitting with no subscribers
    bus.emit(TestEvent { value: 1 }).await;
}

#[tokio::test]
async fn test_emit_and_wait_no_subscribers() {
    let bus = LocalEventBus::new();
    // Should return instantly with no subscribers, no panic
    bus.emit_and_wait(TestEvent { value: 1 }).await;
}

#[tokio::test]
async fn test_default_eventbus() {
    let default_bus = LocalEventBus::default();
    let new_bus = LocalEventBus::new();
    assert_eq!(
        default_bus.concurrency_limit(),
        new_bus.concurrency_limit(),
    );
    assert_eq!(default_bus.concurrency_limit(), Some(DEFAULT_MAX_CONCURRENCY));
}

#[tokio::test]
async fn test_concurrency_limit_bounded() {
    let bus = LocalEventBus::with_concurrency(5);
    assert_eq!(bus.concurrency_limit(), Some(5));
}

#[tokio::test]
async fn test_concurrency_limit_unbounded() {
    let bus = LocalEventBus::unbounded();
    assert_eq!(bus.concurrency_limit(), None);
}

#[tokio::test]
async fn test_clone_shares_state() {
    let bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));

    let c = counter.clone();
    bus.subscribe(move |_: Arc<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
        }
    })
    .await;

    // Clone the bus and emit on the clone
    let bus2 = bus.clone();
    bus2.emit_and_wait(TestEvent { value: 1 }).await;

    // Handler registered on original should have been invoked via clone
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_drop_bus_with_active_handlers() {
    let bus = LocalEventBus::new();
    let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let f = flag.clone();
    bus.subscribe(move |_: Arc<SlowEvent>| {
        let f = f.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            f.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    })
    .await;

    // Fire-and-forget, then immediately drop the bus
    bus.emit(SlowEvent).await;
    drop(bus);

    // Spawned task should still complete despite bus being dropped
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(flag.load(std::sync::atomic::Ordering::SeqCst));
}

// --- Phase 4: Async Handler Behavior ---

#[tokio::test]
async fn test_handler_with_long_sleep() {
    let bus = LocalEventBus::new();
    let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let f = flag.clone();
    bus.subscribe(move |_: Arc<TestEvent>| {
        let f = f.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            f.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    })
    .await;

    // emit() returns after spawning, not after handler completes
    bus.emit(TestEvent { value: 1 }).await;
    assert!(!flag.load(std::sync::atomic::Ordering::SeqCst));

    // After enough time, the handler should have completed
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(flag.load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn test_emit_and_wait_waits_for_slow() {
    let bus = LocalEventBus::new();
    let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let f = flag.clone();
    bus.subscribe(move |_: Arc<TestEvent>| {
        let f = f.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            f.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    })
    .await;

    // emit_and_wait should block until handler completes
    bus.emit_and_wait(TestEvent { value: 1 }).await;
    assert!(flag.load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn test_handler_spawns_nested_emit() {
    let bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));

    // Handler for TestEvent emits an OtherEvent
    let bus2 = bus.clone();
    bus.subscribe(move |_: Arc<TestEvent>| {
        let bus2 = bus2.clone();
        async move {
            bus2.emit(OtherEvent).await;
        }
    })
    .await;

    // Handler for OtherEvent increments counter
    let c = counter.clone();
    bus.subscribe(move |_: Arc<OtherEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
        }
    })
    .await;

    bus.emit_and_wait(TestEvent { value: 1 }).await;
    // Nested emit is fire-and-forget, wait for it
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_handler_shared_state_mutation() {
    let bus = LocalEventBus::new();
    let data = Arc::new(tokio::sync::Mutex::new(Vec::<i32>::new()));

    for i in 1..=3 {
        let d = data.clone();
        bus.subscribe(move |_: Arc<TestEvent>| {
            let d = d.clone();
            async move {
                d.lock().await.push(i);
            }
        })
        .await;
    }

    bus.emit_and_wait(TestEvent { value: 1 }).await;

    let mut result = data.lock().await.clone();
    result.sort();
    assert_eq!(result, vec![1, 2, 3]);
}

// --- Phase 5: Stress & Performance ---

#[tokio::test]
async fn test_stress_many_events() {
    let bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));

    let c = counter.clone();
    bus.subscribe(move |_: Arc<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
        }
    })
    .await;

    for _ in 0..100 {
        bus.emit_and_wait(TestEvent { value: 1 }).await;
    }
    assert_eq!(counter.load(Ordering::SeqCst), 100);
}

#[tokio::test]
async fn test_stress_many_subscribers() {
    let bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));

    for _ in 0..50 {
        let c = counter.clone();
        bus.subscribe(move |_: Arc<TestEvent>| {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
            }
        })
        .await;
    }

    bus.emit_and_wait(TestEvent { value: 1 }).await;
    assert_eq!(counter.load(Ordering::SeqCst), 50);
}

#[tokio::test]
async fn test_stress_concurrent_emit() {
    let bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));

    let c = counter.clone();
    bus.subscribe(move |_: Arc<TestEvent>| {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
        }
    })
    .await;

    let mut handles = Vec::new();
    for _ in 0..10 {
        let bus = bus.clone();
        handles.push(tokio::spawn(async move {
            for _ in 0..10 {
                bus.emit_and_wait(TestEvent { value: 1 }).await;
            }
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    assert_eq!(counter.load(Ordering::SeqCst), 100);
}

#[tokio::test]
async fn test_backpressure_high_load() {
    let bus = LocalEventBus::with_concurrency(2);
    let active = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));

    let active_c = active.clone();
    let max_c = max_seen.clone();
    let completed_c = completed.clone();
    bus.subscribe(move |_: Arc<SlowEvent>| {
        let active = active_c.clone();
        let max_seen = max_c.clone();
        let completed = completed_c.clone();
        async move {
            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
            max_seen.fetch_max(current, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(30)).await;
            active.fetch_sub(1, Ordering::SeqCst);
            completed.fetch_add(1, Ordering::SeqCst);
        }
    })
    .await;

    for _ in 0..20 {
        bus.emit(SlowEvent).await;
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

#[tokio::test]
async fn test_local_event_bus_implements_trait() {
    // Verify LocalEventBus can be used where EventBus trait is expected
    fn assert_event_bus<T: EventBus>(_bus: &T) {}
    let bus = LocalEventBus::new();
    assert_event_bus(&bus);
}
