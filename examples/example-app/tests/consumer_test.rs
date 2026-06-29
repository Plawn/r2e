use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use r2e::prelude::*;
use r2e::r2e_events::LocalEventBus;
// Import the Controller trait explicitly (prelude exports the derive macro with the same name)
use r2e::Controller as ControllerTrait;

// ─── Test event type ───

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct TestConsumerEvent {
    pub message: String,
}

// ─── Test state ───

#[derive(Clone, TestState)]
struct ConsumerTestState {
    pub event_bus: LocalEventBus,
    pub counter: Arc<AtomicUsize>,
    pub received: Arc<tokio::sync::Mutex<Option<String>>>,
}

fn make_state() -> ConsumerTestState {
    ConsumerTestState {
        event_bus: LocalEventBus::new(),
        counter: Arc::new(AtomicUsize::new(0)),
        received: Arc::new(tokio::sync::Mutex::new(None)),
    }
}

// ─── Consumer controllers ───

#[controller(state = ConsumerTestState)]
pub struct CountingConsumer {
    #[inject]
    event_bus: LocalEventBus,
    #[inject]
    counter: Arc<AtomicUsize>,
}

#[routes]
impl CountingConsumer {
    #[consumer(bus = "event_bus")]
    async fn on_event(&self, _event: Arc<TestConsumerEvent>) {
        self.counter.fetch_add(1, Ordering::SeqCst);
    }
}

#[controller(state = ConsumerTestState)]
pub struct DataCapturingConsumer {
    #[inject]
    event_bus: LocalEventBus,
    #[inject]
    received: Arc<tokio::sync::Mutex<Option<String>>>,
}

#[routes]
impl DataCapturingConsumer {
    #[consumer(bus = "event_bus")]
    async fn on_event(&self, event: Arc<TestConsumerEvent>) {
        *self.received.lock().await = Some(event.message.clone());
    }
}

#[controller(state = ConsumerTestState)]
pub struct SecondCountingConsumer {
    #[inject]
    event_bus: LocalEventBus,
    #[inject]
    counter: Arc<AtomicUsize>,
}

#[routes]
impl SecondCountingConsumer {
    #[consumer(bus = "event_bus")]
    async fn on_event(&self, _event: Arc<TestConsumerEvent>) {
        self.counter.fetch_add(1, Ordering::SeqCst);
    }
}

struct CloneTrackedConsumerDep {
    clones: Arc<AtomicUsize>,
    handled: Arc<AtomicUsize>,
}

impl Clone for CloneTrackedConsumerDep {
    fn clone(&self) -> Self {
        self.clones.fetch_add(1, Ordering::SeqCst);
        Self {
            clones: Arc::clone(&self.clones),
            handled: Arc::clone(&self.handled),
        }
    }
}

struct ReuseConsumerState {
    event_bus: LocalEventBus,
    dep: CloneTrackedConsumerDep,
}

impl Clone for ReuseConsumerState {
    fn clone(&self) -> Self {
        Self {
            event_bus: self.event_bus.clone(),
            dep: CloneTrackedConsumerDep {
                clones: Arc::clone(&self.dep.clones),
                handled: Arc::clone(&self.dep.handled),
            },
        }
    }
}

#[controller(state = ReuseConsumerState)]
struct ReuseConsumer {
    #[inject]
    event_bus: LocalEventBus,
    #[inject]
    dep: CloneTrackedConsumerDep,
}

#[routes]
impl ReuseConsumer {
    #[consumer(bus = "event_bus")]
    async fn on_event(&self, _event: Arc<TestConsumerEvent>) {
        self.dep.handled.fetch_add(1, Ordering::SeqCst);
    }
}

// ─── Tests ───

#[r2e::test]
async fn test_consumer_method_invoked() {
    let state = make_state();
    let core = Arc::new(CountingConsumer::from_state(&state));

    // Register macro-generated consumer
    <CountingConsumer as ControllerTrait<ConsumerTestState>>::register_consumers(
        state.clone(),
        core,
    )
    .await;

    // Emit event
    let _ = state
        .event_bus
        .emit(TestConsumerEvent {
            message: "hello".into(),
        })
        .await;

    // Wait for async consumer to process
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(state.counter.load(Ordering::SeqCst), 1);
}

#[r2e::test]
async fn test_consumer_receives_correct_data() {
    let state = make_state();
    let core = Arc::new(DataCapturingConsumer::from_state(&state));

    <DataCapturingConsumer as ControllerTrait<ConsumerTestState>>::register_consumers(
        state.clone(),
        core,
    )
    .await;

    let _ = state
        .event_bus
        .emit(TestConsumerEvent {
            message: "important payload".into(),
        })
        .await;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let captured = state.received.lock().await;
    assert_eq!(captured.as_deref(), Some("important payload"));
}

#[r2e::test]
async fn test_consumer_with_injected_deps() {
    let state = make_state();
    let core = Arc::new(CountingConsumer::from_state(&state));

    // Register CountingConsumer — its handler uses the injected `counter` dep
    <CountingConsumer as ControllerTrait<ConsumerTestState>>::register_consumers(
        state.clone(),
        core,
    )
    .await;

    // Emit multiple events to verify injected state is correctly shared
    for _ in 0..5 {
        let _ = state
            .event_bus
            .emit(TestConsumerEvent {
                message: "tick".into(),
            })
            .await;
    }

    tokio::time::sleep(Duration::from_millis(200)).await;

    // The injected counter should have been incremented by each invocation
    assert_eq!(state.counter.load(Ordering::SeqCst), 5);
}

#[r2e::test]
async fn test_multiple_consumers_same_event() {
    let state = make_state();
    let first_core = Arc::new(CountingConsumer::from_state(&state));
    let second_core = Arc::new(SecondCountingConsumer::from_state(&state));

    // Register two different consumer controllers for the same event type
    <CountingConsumer as ControllerTrait<ConsumerTestState>>::register_consumers(
        state.clone(),
        first_core,
    )
    .await;
    <SecondCountingConsumer as ControllerTrait<ConsumerTestState>>::register_consumers(
        state.clone(),
        second_core,
    )
    .await;

    let _ = state
        .event_bus
        .emit(TestConsumerEvent {
            message: "broadcast".into(),
        })
        .await;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Both consumers increment the same counter, so it should be 2
    assert_eq!(state.counter.load(Ordering::SeqCst), 2);
}

#[r2e::test]
async fn consumer_reuses_supplied_core_for_every_event() {
    let clones = Arc::new(AtomicUsize::new(0));
    let handled = Arc::new(AtomicUsize::new(0));
    let state = ReuseConsumerState {
        event_bus: LocalEventBus::new(),
        dep: CloneTrackedConsumerDep {
            clones: Arc::clone(&clones),
            handled: Arc::clone(&handled),
        },
    };
    let core = Arc::new(ReuseConsumer::from_state(&state));
    assert_eq!(clones.load(Ordering::SeqCst), 1);

    <ReuseConsumer as ControllerTrait<ReuseConsumerState>>::register_consumers(state.clone(), core)
        .await;

    for _ in 0..5 {
        let _ = state
            .event_bus
            .emit(TestConsumerEvent {
                message: "reuse".into(),
            })
            .await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(handled.load(Ordering::SeqCst), 5);
    assert_eq!(
        clones.load(Ordering::SeqCst),
        1,
        "consumer events must not reconstruct the controller core"
    );
}
