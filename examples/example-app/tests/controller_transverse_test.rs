//! W10 phase 3: controller cores reuse the bean-level transverse machinery —
//! `#[post_construct]` lifecycle hooks and `#[intercept]` on `#[consumer]`
//! methods (method-level + impl-level, responders included), with direct-call
//! interception through the core's decorator slot.
//!
//! These exercise the real registration/boot path: `register_controller`
//! (fills the slot via `fill_decos`, queues post-construct + consumers) then
//! `build_with_consumers` (runs post-construct BEFORE consumers).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use r2e::prelude::*;
use r2e::r2e_events::{EventBus, LocalEventBus};
use r2e::BeanContext;
use r2e::Controller as ControllerTrait;

// ─── Evidence + a bean-reading interceptor spec (non-TNil Deps) ───

#[derive(Clone, Default)]
pub struct Evidence {
    entries: Arc<Mutex<Vec<String>>>,
}

impl Evidence {
    fn record(&self, s: String) {
        self.entries.lock().unwrap().push(s);
    }
    fn snapshot(&self) -> Vec<String> {
        self.entries.lock().unwrap().clone()
    }
}

#[derive(DecoratorBean)]
pub struct Audit {
    #[inject]
    evidence: Evidence,
    tag: &'static str,
}

impl<R: Send> Interceptor<R> for Audit {
    fn around<F, Fut>(
        &self,
        ctx: InterceptorContext,
        next: F,
    ) -> impl std::future::Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = R> + Send,
    {
        let tag = self.tag;
        let method = ctx.method_name;
        let ev = self.evidence.clone();
        async move {
            ev.record(format!("{tag}:{method}"));
            next().await
        }
    }
}

// ─── Helper: call `fill_decos` letting the compiler infer the witness `W`
// (same pattern as `RegisterController`). Used for the direct-call test, which
// needs the slot filled without going through `register_controller`. ───

trait FillExt<S, W>: Sized {
    fn fill_slot(state: &S, core: &Arc<Self>, ctx: &BeanContext);
}

impl<C, S, W> FillExt<S, W> for C
where
    C: ControllerTrait<S, W>,
    S: Clone + Send + Sync + 'static,
{
    fn fill_slot(_state: &S, core: &Arc<Self>, ctx: &BeanContext) {
        <C as ControllerTrait<S, W>>::fill_decos(core, ctx);
    }
}

// ─── Events ───

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Ping {
    #[allow(dead_code)]
    msg: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Question {
    q: String,
}

// ─── post_construct controller (runs before consumers) ───

#[controller]
pub struct LifecycleController {
    #[inject]
    log: Arc<Mutex<Vec<String>>>,
    #[inject]
    event_bus: LocalEventBus,
}

#[routes]
impl LifecycleController {
    #[post_construct]
    async fn init(&self) {
        self.log.lock().unwrap().push("post_construct".into());
    }

    #[consumer(bus = "event_bus")]
    async fn on_ping(&self, _e: Arc<Ping>) {
        self.log.lock().unwrap().push("consumer".into());
    }
}

// ─── Controller whose post_construct fails (must abort startup) ───

#[controller]
pub struct FailingLifecycle {
    #[inject]
    marker: Arc<AtomicUsize>,
}

#[routes]
impl FailingLifecycle {
    #[post_construct]
    async fn boom(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.marker.fetch_add(1, Ordering::SeqCst);
        Err("post_construct boom".into())
    }
}

// ─── Intercepted consumer controller: impl-level + method-level ───

#[controller]
pub struct PingController {
    #[inject]
    event_bus: LocalEventBus,
    #[inject]
    seen: Arc<AtomicUsize>,
}

#[routes]
#[intercept(Audit::spec("impl"))]
impl PingController {
    #[consumer(bus = "event_bus")]
    #[intercept(Audit::spec("method"))]
    async fn on_ping(&self, _e: Arc<Ping>) {
        self.seen.fetch_add(1, Ordering::SeqCst);
    }
}

// ─── Intercepted responder controller ───

#[controller]
pub struct AnswerController {
    #[inject]
    event_bus: LocalEventBus,
}

#[routes]
impl AnswerController {
    #[consumer(bus = "event_bus")]
    #[intercept(Audit::spec("resp"))]
    async fn answer(&self, q: Arc<Question>) -> String {
        format!("answer:{}", q.q)
    }
}

// ─── Tests ───

#[r2e::test]
async fn controller_post_construct_runs_before_consumer_traffic() {
    let log = Arc::new(Mutex::new(Vec::<String>::new()));
    let bus = LocalEventBus::new();

    let _router = AppBuilder::new()
        .provide(log.clone())
        .provide(bus.clone())
        .build_state()
        .await
        .register_controller::<LifecycleController>()
        .build_with_consumers()
        .await;

    // At boot, post_construct has run; the consumer is subscribed but no event
    // has flowed yet.
    assert_eq!(
        *log.lock().unwrap(),
        vec!["post_construct".to_string()],
        "post_construct must run at startup, before any consumer traffic"
    );

    bus.emit(Ping { msg: "hi".into() }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    assert_eq!(
        *log.lock().unwrap(),
        vec!["post_construct".to_string(), "consumer".to_string()],
        "consumer runs after post_construct"
    );
}

#[r2e::test]
#[should_panic(expected = "post_construct")]
async fn controller_post_construct_error_fails_startup() {
    let marker = Arc::new(AtomicUsize::new(0));
    // build_with_consumers returns a Router, so a failing post_construct panics.
    let _router = AppBuilder::new()
        .provide(marker.clone())
        .build_state()
        .await
        .register_controller::<FailingLifecycle>()
        .build_with_consumers()
        .await;
}

#[r2e::test]
async fn controller_consumer_intercept_impl_then_method() {
    let bus = LocalEventBus::new();
    let seen = Arc::new(AtomicUsize::new(0));
    let evidence = Evidence::default();

    let _router = AppBuilder::new()
        .provide(bus.clone())
        .provide(seen.clone())
        .provide(evidence.clone())
        .build_state()
        .await
        .register_controller::<PingController>()
        .build_with_consumers()
        .await;

    bus.emit(Ping { msg: "hi".into() }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    assert_eq!(seen.load(Ordering::SeqCst), 1);
    // impl-level interceptor runs BEFORE method-level (same order as beans).
    assert_eq!(
        evidence.snapshot(),
        vec!["impl:on_ping".to_string(), "method:on_ping".to_string()],
        "impl-level before method-level",
    );
}

#[r2e::test]
async fn controller_responder_intercept_and_reply_flows() {
    let bus = LocalEventBus::new();
    let evidence = Evidence::default();

    let _router = AppBuilder::new()
        .provide(bus.clone())
        .provide(evidence.clone())
        .build_state()
        .await
        .register_controller::<AnswerController>()
        .build_with_consumers()
        .await;

    let reply: String = bus
        .request::<Question, String>(Question { q: "life".into() })
        .await
        .expect("responder replies");
    assert_eq!(reply, "answer:life");
    assert_eq!(evidence.snapshot(), vec!["resp:answer".to_string()]);
}

#[r2e::test]
async fn controller_consumer_direct_call_is_intercepted() {
    // A registered core's dispatch wrapper reads the filled slot, so a DIRECT
    // in-code call runs the chain too (not just event delivery).
    let bus = LocalEventBus::new();
    let seen = Arc::new(AtomicUsize::new(0));
    let evidence = Evidence::default();

    let app = AppBuilder::new()
        .provide(bus.clone())
        .provide(seen.clone())
        .provide(evidence.clone())
        .build_state()
        .await;

    let core = Arc::new(PingController::from_context(app.bean_context()));
    // Fill the slot (the registration step) without event wiring.
    PingController::fill_slot(app.state(), &core, app.bean_context());

    core.on_ping(Arc::new(Ping { msg: "direct".into() })).await;

    assert_eq!(seen.load(Ordering::SeqCst), 1);
    assert_eq!(
        evidence.snapshot(),
        vec!["impl:on_ping".to_string(), "method:on_ping".to_string()],
    );
}

#[r2e::test]
async fn controller_consumer_unregistered_core_is_undecorated() {
    // A core built via from_context but never registered has an empty slot: the
    // body runs, the chain does not.
    let bus = LocalEventBus::new();
    let seen = Arc::new(AtomicUsize::new(0));
    let evidence = Evidence::default();

    let app = AppBuilder::new()
        .provide(bus.clone())
        .provide(seen.clone())
        .provide(evidence.clone())
        .build_state()
        .await;

    let core = PingController::from_context(app.bean_context());
    core.on_ping(Arc::new(Ping { msg: "direct".into() })).await;

    assert_eq!(seen.load(Ordering::SeqCst), 1, "body must run");
    assert!(
        evidence.snapshot().is_empty(),
        "unregistered core must not intercept: {:?}",
        evidence.snapshot()
    );
}
