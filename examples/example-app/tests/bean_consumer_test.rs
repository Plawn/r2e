//! `#[consumer]` on `#[bean]` — auto-collection at `build_state()` (W10
//! follow-up).
//!
//! Beans declare event consumers with the same `#[consumer]` attribute as
//! controllers; `#[bean]` generates an `EventSubscriber` impl and an
//! `after_register` hook, so `.register::<T>()` alone is enough:
//! `build_state()` queues the subscription and it runs at server startup
//! (`serve` / `build_with_consumers`) — no explicit `register_subscriber`
//! call (the method no longer exists).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use r2e::prelude::*;
use r2e::r2e_events::{EventBus, LocalEventBus};

// ─── Fan-out consumer bean ───

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Ping {
    #[allow(dead_code)]
    msg: String,
}

#[derive(Clone)]
pub struct PingConsumer {
    bus: LocalEventBus,
    seen: Arc<AtomicUsize>,
}

#[bean]
impl PingConsumer {
    pub fn new(bus: LocalEventBus, seen: Arc<AtomicUsize>) -> Self {
        Self { bus, seen }
    }

    #[consumer(bus = "bus")]
    async fn on_ping(&self, _e: Arc<Ping>) {
        self.seen.fetch_add(1, Ordering::SeqCst);
    }
}

// ─── Responder consumer bean ───

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Question {
    q: String,
}

#[derive(Clone)]
pub struct Answerer {
    bus: LocalEventBus,
}

#[bean]
impl Answerer {
    pub fn new(bus: LocalEventBus) -> Self {
        Self { bus }
    }

    #[consumer(bus = "bus")]
    async fn answer(&self, q: Arc<Question>) -> String {
        format!("answer:{}", q.q)
    }
}

// ─── Tests ───

#[r2e::test]
async fn bean_consumers_auto_subscribe_at_startup() {
    let bus = LocalEventBus::new();
    let seen = Arc::new(AtomicUsize::new(0));
    let _router = AppBuilder::new()
        .provide(bus.clone())
        .provide(seen.clone())
        .register::<PingConsumer>()
        .build_state()
        .await
        // Runs the consumer registrations that `serve()` would run at startup.
        .build_with_consumers()
        .await;

    bus.emit(Ping { msg: "hi".into() }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    assert_eq!(
        seen.load(Ordering::SeqCst),
        1,
        "the registered bean's #[consumer] subscribed without any explicit call"
    );
}

#[r2e::test]
async fn bean_consumers_do_not_subscribe_at_build_state_alone() {
    // The subscription is a startup action (consumer registration), not a
    // build_state side effect — same timing as controller #[consumer] methods.
    let bus = LocalEventBus::new();
    let seen = Arc::new(AtomicUsize::new(0));
    let _app = AppBuilder::new()
        .provide(bus.clone())
        .provide(seen.clone())
        .register::<PingConsumer>()
        .build_state()
        .await;

    bus.emit(Ping { msg: "early".into() }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    assert_eq!(seen.load(Ordering::SeqCst), 0);
}

#[r2e::test]
async fn bean_responder_auto_subscribes() {
    let bus = LocalEventBus::new();
    let _router = AppBuilder::new()
        .provide(bus.clone())
        .register::<Answerer>()
        .build_state()
        .await
        .build_with_consumers()
        .await;

    let reply: String = bus
        .request::<Question, String>(Question { q: "life".into() })
        .await
        .expect("responder replies");
    assert_eq!(reply, "answer:life");
}

#[r2e::test]
async fn provided_instance_does_not_auto_subscribe() {
    // The after_register hook only runs on `.register::<T>()` — a bean
    // deposited via `.provide(instance)` never queues a subscription (mirror
    // of `#[scheduled]` sources). Escape hatch: add_consumer_registration.
    let bus = LocalEventBus::new();
    let seen = Arc::new(AtomicUsize::new(0));
    let _router = AppBuilder::new()
        .provide(bus.clone())
        .provide(PingConsumer::new(bus.clone(), seen.clone()))
        .build_state()
        .await
        .build_with_consumers()
        .await;

    bus.emit(Ping { msg: "provided".into() }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    assert_eq!(
        seen.load(Ordering::SeqCst),
        0,
        "a provided #[consumer] bean must not auto-subscribe — register the \
         type instead"
    );
}

#[r2e::test]
async fn default_override_registration_subscribes_once() {
    // `with_default_bean` + `register_override` of the SAME type run
    // `after_register` twice; the subscriber hook must stay unique per type or
    // every event would be handled twice.
    let bus = LocalEventBus::new();
    let seen = Arc::new(AtomicUsize::new(0));
    let _router = AppBuilder::new()
        .provide(bus.clone())
        .provide(seen.clone())
        .with_default_bean::<PingConsumer>()
        .register_override::<PingConsumer>()
        .build_state()
        .await
        .build_with_consumers()
        .await;

    bus.emit(Ping { msg: "once".into() }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    assert_eq!(
        seen.load(Ordering::SeqCst),
        1,
        "one hook per bean type — events must not be handled twice by the \
         default/override pattern"
    );
}
