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

#[derive(Clone)]
struct ConsumerTestState {
    pub event_bus: LocalEventBus,
    pub counter: Arc<AtomicUsize>,
    pub received: Arc<tokio::sync::Mutex<Option<String>>>,
}

impl r2e::http::extract::FromRef<ConsumerTestState> for LocalEventBus {
    fn from_ref(state: &ConsumerTestState) -> Self {
        state.event_bus.clone()
    }
}

impl r2e::http::extract::FromRef<ConsumerTestState> for Arc<AtomicUsize> {
    fn from_ref(state: &ConsumerTestState) -> Self {
        state.counter.clone()
    }
}

impl r2e::http::extract::FromRef<ConsumerTestState> for Arc<tokio::sync::Mutex<Option<String>>> {
    fn from_ref(state: &ConsumerTestState) -> Self {
        state.received.clone()
    }
}

fn make_state() -> ConsumerTestState {
    ConsumerTestState {
        event_bus: LocalEventBus::new(),
        counter: Arc::new(AtomicUsize::new(0)),
        received: Arc::new(tokio::sync::Mutex::new(None)),
    }
}

// ─── Consumer controllers ───

#[derive(Controller)]
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

#[derive(Controller)]
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

#[derive(Controller)]
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

// ─── Tests ───

#[tokio::test]
async fn test_consumer_method_invoked() {
    let state = make_state();

    // Register macro-generated consumer
    <CountingConsumer as ControllerTrait<ConsumerTestState>>::register_consumers(state.clone()).await;

    // Emit event
    state.event_bus.emit(TestConsumerEvent {
        message: "hello".into(),
    }).await;

    // Wait for async consumer to process
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(state.counter.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_consumer_receives_correct_data() {
    let state = make_state();

    <DataCapturingConsumer as ControllerTrait<ConsumerTestState>>::register_consumers(state.clone()).await;

    state.event_bus.emit(TestConsumerEvent {
        message: "important payload".into(),
    }).await;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let captured = state.received.lock().await;
    assert_eq!(captured.as_deref(), Some("important payload"));
}

#[tokio::test]
async fn test_consumer_with_injected_deps() {
    let state = make_state();

    // Register CountingConsumer — its handler uses the injected `counter` dep
    <CountingConsumer as ControllerTrait<ConsumerTestState>>::register_consumers(state.clone()).await;

    // Emit multiple events to verify injected state is correctly shared
    for _ in 0..5 {
        state.event_bus.emit(TestConsumerEvent {
            message: "tick".into(),
        }).await;
    }

    tokio::time::sleep(Duration::from_millis(200)).await;

    // The injected counter should have been incremented by each invocation
    assert_eq!(state.counter.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn test_multiple_consumers_same_event() {
    let state = make_state();

    // Register two different consumer controllers for the same event type
    <CountingConsumer as ControllerTrait<ConsumerTestState>>::register_consumers(state.clone()).await;
    <SecondCountingConsumer as ControllerTrait<ConsumerTestState>>::register_consumers(state.clone()).await;

    state.event_bus.emit(TestConsumerEvent {
        message: "broadcast".into(),
    }).await;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Both consumers increment the same counter, so it should be 2
    assert_eq!(state.counter.load(Ordering::SeqCst), 2);
}
