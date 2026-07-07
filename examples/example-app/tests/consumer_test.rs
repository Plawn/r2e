use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use r2e::prelude::*;
use r2e::r2e_events::LocalEventBus;
// Import the Controller trait explicitly (prelude exports the derive macro with the same name)
use r2e::Controller as ControllerTrait;

// ─── Helper: call the generated `register_consumers` while letting the compiler
// infer the extraction-marker witness `W` (same pattern as `RegisterController`).
// In the state-generic model the `Controller<S, W>` impl carries opaque
// extraction markers in `W`, so a fully-qualified `<C as Controller<S>>::…`
// call no longer resolves. ───

trait ConsumerExt<S, W>: Sized {
    fn start_consumers(state: S, core: Arc<Self>) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

impl<C, S, W> ConsumerExt<S, W> for C
where
    C: ControllerTrait<S, W>,
    S: Clone + Send + Sync + 'static,
{
    fn start_consumers(state: S, core: Arc<Self>) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        <C as ControllerTrait<S, W>>::register_consumers(state, core)
    }
}

// ─── Test event type ───

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct TestConsumerEvent {
    pub message: String,
}

// ─── Consumer controllers ───

#[controller]
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

#[controller]
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

#[controller]
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

#[controller]
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
    let event_bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));
    let builder = AppBuilder::new()
        .provide(event_bus.clone())
        .provide(counter.clone())
        .build_state()
        .await;
    let core = Arc::new(CountingConsumer::from_context(builder.bean_context()));

    // Register macro-generated consumer
    CountingConsumer::start_consumers(builder.state().clone(), core).await;

    // Emit event
    let _ = event_bus
        .emit(TestConsumerEvent {
            message: "hello".into(),
        })
        .await;

    // Wait for async consumer to process
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[r2e::test]
async fn test_consumer_receives_correct_data() {
    let event_bus = LocalEventBus::new();
    let received: Arc<tokio::sync::Mutex<Option<String>>> =
        Arc::new(tokio::sync::Mutex::new(None));
    let builder = AppBuilder::new()
        .provide(event_bus.clone())
        .provide(received.clone())
        .build_state()
        .await;
    let core = Arc::new(DataCapturingConsumer::from_context(builder.bean_context()));

    DataCapturingConsumer::start_consumers(builder.state().clone(), core).await;

    let _ = event_bus
        .emit(TestConsumerEvent {
            message: "important payload".into(),
        })
        .await;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let captured = received.lock().await;
    assert_eq!(captured.as_deref(), Some("important payload"));
}

#[r2e::test]
async fn test_consumer_with_injected_deps() {
    let event_bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));
    let builder = AppBuilder::new()
        .provide(event_bus.clone())
        .provide(counter.clone())
        .build_state()
        .await;
    let core = Arc::new(CountingConsumer::from_context(builder.bean_context()));

    // Register CountingConsumer — its handler uses the injected `counter` dep
    CountingConsumer::start_consumers(builder.state().clone(), core).await;

    // Emit multiple events to verify injected state is correctly shared
    for _ in 0..5 {
        let _ = event_bus
            .emit(TestConsumerEvent {
                message: "tick".into(),
            })
            .await;
    }

    tokio::time::sleep(Duration::from_millis(200)).await;

    // The injected counter should have been incremented by each invocation
    assert_eq!(counter.load(Ordering::SeqCst), 5);
}

#[r2e::test]
async fn test_multiple_consumers_same_event() {
    let event_bus = LocalEventBus::new();
    let counter = Arc::new(AtomicUsize::new(0));
    let builder = AppBuilder::new()
        .provide(event_bus.clone())
        .provide(counter.clone())
        .build_state()
        .await;
    let first_core = Arc::new(CountingConsumer::from_context(builder.bean_context()));
    let second_core = Arc::new(SecondCountingConsumer::from_context(builder.bean_context()));

    // Register two different consumer controllers for the same event type
    CountingConsumer::start_consumers(builder.state().clone(), first_core).await;
    SecondCountingConsumer::start_consumers(builder.state().clone(), second_core).await;

    let _ = event_bus
        .emit(TestConsumerEvent {
            message: "broadcast".into(),
        })
        .await;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Both consumers increment the same counter, so it should be 2
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[r2e::test]
async fn consumer_reuses_supplied_core_for_every_event() {
    let clones = Arc::new(AtomicUsize::new(0));
    let handled = Arc::new(AtomicUsize::new(0));
    let event_bus = LocalEventBus::new();
    let dep = CloneTrackedConsumerDep {
        clones: Arc::clone(&clones),
        handled: Arc::clone(&handled),
    };

    let builder = AppBuilder::new()
        .provide(event_bus.clone())
        .provide(dep)
        .build_state()
        .await;
    let core = Arc::new(ReuseConsumer::from_context(builder.bean_context()));

    // Pass a dep-free state to `register_consumers` (the state argument is
    // unused by the generated registration) so the clone counter tracks only
    // core (re)construction, not incidental state clones.
    let empty_builder = AppBuilder::new().build_state().await;
    let empty_state = empty_builder.state().clone();

    ReuseConsumer::start_consumers(empty_state, core).await;

    // Baseline: all dep clones from building state + constructing the core have
    // happened by now. Handling events must not add more.
    let base = clones.load(Ordering::SeqCst);

    for _ in 0..5 {
        let _ = event_bus
            .emit(TestConsumerEvent {
                message: "reuse".into(),
            })
            .await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(handled.load(Ordering::SeqCst), 5);
    assert_eq!(
        clones.load(Ordering::SeqCst),
        base,
        "consumer events must not reconstruct the controller core"
    );
}
