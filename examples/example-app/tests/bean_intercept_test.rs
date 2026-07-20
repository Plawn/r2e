//! `#[intercept(...)]` on `#[bean]` `#[scheduled]` / `#[consumer]` methods — W10 phase 2.
//!
//! Interceptors are built ONCE at registration from the resolved bean graph
//! (`DecoratorSpec::build`), stored in the bean's shared decorator slot
//! (injected by `#[bean]` on the struct), and run inside each method's
//! dispatch wrapper — so scheduler ticks / event deliveries AND direct in-code
//! calls all go through the chain.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use r2e::builder::ScheduledTaskMarker;
use r2e::prelude::*;
use r2e::r2e_events::{EventBus, LocalEventBus};
use r2e::r2e_executor::{Executor, ExecutorConfig, PoolExecutor};
use r2e::r2e_scheduler::{
    extract_tasks, start_jobs, ScheduledJobRegistry, Scheduler, SchedulerCommands,
};
use r2e::{Decorate, TaskRegistryHandle};
use tokio_util::sync::CancellationToken;

// ─── Evidence bean + a bean-reading interceptor spec (non-TNil Deps) ───

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

/// A `#[derive(DecoratorBean)]` interceptor that reads the `Evidence` bean from
/// the graph (so its `Deps` is non-empty) and records `tag:method` per call.
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

// ─── Scheduled bean with intercepted (async + sync) methods ───

#[bean]
#[derive(Clone)]
pub struct SchedTicker {
    ticks: Arc<AtomicUsize>,
}

#[bean]
impl SchedTicker {
    pub fn new(ticks: Arc<AtomicUsize>) -> Self {
        Self { ticks }
    }

    #[scheduled(every = 1)]
    #[intercept(Audit::spec("tick"))]
    async fn tick(&self) {
        self.ticks.fetch_add(1, Ordering::SeqCst);
    }

    // Sync scheduled source: its wrapper is promoted to `async fn` because it
    // has an `#[intercept]` site — call sites `.await` it.
    #[scheduled(every = 3600, name = "sync_bean_tick")]
    #[intercept(Audit::spec("synctick"))]
    fn sync_tick(&self) {
        self.ticks.fetch_add(1, Ordering::SeqCst);
    }
}

// ─── Fan-out consumer bean with impl-level + method-level interceptors ───

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Ping {
    #[allow(dead_code)]
    msg: String,
}

#[bean]
#[derive(Clone)]
pub struct PingConsumer {
    bus: LocalEventBus,
    seen: Arc<AtomicUsize>,
}

#[bean]
#[intercept(Audit::spec("impl"))]
impl PingConsumer {
    pub fn new(bus: LocalEventBus, seen: Arc<AtomicUsize>) -> Self {
        Self { bus, seen }
    }

    #[consumer(bus = "bus")]
    #[intercept(Audit::spec("method"))]
    async fn on_ping(&self, _e: Arc<Ping>) {
        self.seen.fetch_add(1, Ordering::SeqCst);
    }
}

// ─── Responder consumer bean ───

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Question {
    q: String,
}

#[bean]
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
    #[intercept(Audit::spec("resp"))]
    async fn answer(&self, q: Arc<Question>) -> String {
        format!("answer:{}", q.q)
    }
}

// ─── Bean with a Result-erroring intercepted scheduled method ───

#[bean]
#[derive(Clone)]
pub struct FailingBean {
    calls: Arc<AtomicUsize>,
}

#[bean]
impl FailingBean {
    pub fn new(calls: Arc<AtomicUsize>) -> Self {
        Self { calls }
    }

    #[scheduled(every = 3600, name = "failing_tick")]
    #[intercept(Audit::spec("fail"))]
    async fn failing(&self) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err("boom".into())
    }
}

// ─── Bean B that holds a SchedTicker clone (captured during resolution,
// BEFORE the decorator slot is filled) and calls through it. ───

// No `#[bean]` on the struct: Caller has no intercept sites of its own, so it
// needs no decorator slot.
#[derive(Clone)]
pub struct Caller {
    ticker: SchedTicker,
}

#[bean]
impl Caller {
    pub fn new(ticker: SchedTicker) -> Self {
        Self { ticker }
    }

    async fn poke(&self) {
        // Direct call through a clone the container bean captured at
        // construction time — the shared slot must be filled by now.
        self.ticker.tick().await;
    }
}

// ─── Tests ───

#[r2e::test]
async fn scheduled_interceptor_fires_on_ticks() {
    let ticks = Arc::new(AtomicUsize::new(0));
    let evidence = Evidence::default();
    let app = AppBuilder::new()
        .plugin(Executor)
        .plugin(Scheduler)
        .provide(ticks.clone())
        .provide(evidence.clone())
        .register::<SchedTicker>()
        .build_state()
        .await;

    let registry = app
        .get_plugin_data::<TaskRegistryHandle>()
        .expect("scheduler registry present");
    let tasks = extract_tasks(registry.take_of::<ScheduledTaskMarker>());
    assert_eq!(tasks.len(), 2, "two scheduled methods collected");

    let cancel = CancellationToken::new();
    let pool = PoolExecutor::new(ExecutorConfig::default());
    let jobs: Vec<_> = tasks.into_iter().map(|t| t.into_job()).collect();
    start_jobs(
        jobs,
        cancel.clone(),
        pool,
        ScheduledJobRegistry::new(),
        SchedulerCommands::disconnected(),
    );
    tokio::time::sleep(Duration::from_millis(2500)).await;
    cancel.cancel();

    assert!(ticks.load(Ordering::SeqCst) >= 2);
    let entries = evidence.snapshot();
    let tick_hits = entries.iter().filter(|e| *e == "tick:tick").count();
    assert!(
        tick_hits >= 2,
        "interceptor should fire per tick, got {entries:?}"
    );
    // The 1h sync task fires its initial tick once → one "synctick:sync_tick".
    assert!(
        entries.iter().any(|e| e == "synctick:sync_tick"),
        "{entries:?}"
    );
}

#[r2e::test]
async fn scheduled_interceptor_fires_on_direct_calls() {
    let ticks = Arc::new(AtomicUsize::new(0));
    let evidence = Evidence::default();
    // No scheduler needed: the slot is filled during build_state() regardless.
    let app = AppBuilder::new()
        .provide(ticks.clone())
        .provide(evidence.clone())
        .register::<SchedTicker>()
        .build_state()
        .await;

    let bean: SchedTicker = app.bean_context().get();
    // Async source method — promoted wrapper self-intercepts.
    bean.tick().await;
    // Sync source method — promoted to async, call with `.await`.
    bean.sync_tick().await;

    assert_eq!(ticks.load(Ordering::SeqCst), 2);
    let entries = evidence.snapshot();
    assert_eq!(
        entries,
        vec!["tick:tick", "synctick:sync_tick"],
        "{entries:?}"
    );
}

#[r2e::test]
async fn consumer_interceptor_fires_impl_then_method() {
    let bus = LocalEventBus::new();
    let seen = Arc::new(AtomicUsize::new(0));
    let evidence = Evidence::default();
    let app = AppBuilder::new()
        .provide(bus.clone())
        .provide(seen.clone())
        .provide(evidence.clone())
        .register::<PingConsumer>()
        .build_state()
        .await;

    let bean: PingConsumer = app.bean_context().get();
    bean.subscribe().await;

    bus.emit(Ping { msg: "hi".into() }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    assert_eq!(seen.load(Ordering::SeqCst), 1);
    let entries = evidence.snapshot();
    // impl-level interceptor runs BEFORE method-level.
    assert_eq!(
        entries,
        vec!["impl:on_ping", "method:on_ping"],
        "impl-level before method-level; got {entries:?}"
    );
}

#[r2e::test]
async fn responder_interceptor_fires_and_reply_flows() {
    let bus = LocalEventBus::new();
    let evidence = Evidence::default();
    let app = AppBuilder::new()
        .provide(bus.clone())
        .provide(evidence.clone())
        .register::<Answerer>()
        .build_state()
        .await;

    let bean: Answerer = app.bean_context().get();
    bean.subscribe().await;

    let reply: String = bus
        .request::<Question, String>(Question { q: "life".into() })
        .await
        .expect("responder replies");
    assert_eq!(reply, "answer:life");

    let entries = evidence.snapshot();
    assert_eq!(entries, vec!["resp:answer"], "{entries:?}");
}

#[r2e::test]
async fn direct_call_on_registered_consumer_bean_is_intercepted() {
    let bus = LocalEventBus::new();
    let seen = Arc::new(AtomicUsize::new(0));
    let evidence = Evidence::default();
    let app = AppBuilder::new()
        .provide(bus.clone())
        .provide(seen.clone())
        .provide(evidence.clone())
        .register::<PingConsumer>()
        .build_state()
        .await;

    let bean: PingConsumer = app.bean_context().get();
    // Direct in-code call goes through the same dispatch wrapper.
    bean.on_ping(Arc::new(Ping {
        msg: "direct".into(),
    }))
    .await;

    assert_eq!(seen.load(Ordering::SeqCst), 1);
    assert_eq!(evidence.snapshot(), vec!["impl:on_ping", "method:on_ping"],);
}

#[r2e::test]
async fn intercepted_method_returning_err_still_fires_and_flows() {
    let calls = Arc::new(AtomicUsize::new(0));
    let evidence = Evidence::default();
    let app = AppBuilder::new()
        .provide(calls.clone())
        .provide(evidence.clone())
        .register::<FailingBean>()
        .build_state()
        .await;

    let bean: FailingBean = app.bean_context().get();
    // The wrapper's Result flows out unchanged, and the interceptor fired.
    let result = bean.failing().await;
    assert_eq!(result, Err("boom".to_string()));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(evidence.snapshot(), vec!["fail:failing"]);
}

#[r2e::test]
async fn two_intercepted_beans_co_resolved_in_one_app() {
    // Multiple deco_fills in ONE resolve, multiple containers from ONE
    // BeanContext — every chain must fire. SchedTicker (`ticks`) and
    // PingConsumer (`seen`) both inject `Arc<AtomicUsize>`, so a single shared
    // provision satisfies both (two values of one type would collide).
    let counter = Arc::new(AtomicUsize::new(0));
    let bus = LocalEventBus::new();
    let evidence = Evidence::default();
    let app = AppBuilder::new()
        .provide(counter.clone())
        .provide(bus.clone())
        .provide(evidence.clone())
        .register::<SchedTicker>()
        .register::<PingConsumer>()
        .register::<Answerer>()
        .build_state()
        .await;

    let ticker: SchedTicker = app.bean_context().get();
    ticker.tick().await;

    let consumer: PingConsumer = app.bean_context().get();
    consumer.subscribe().await;
    let answerer: Answerer = app.bean_context().get();
    answerer.subscribe().await;

    bus.emit(Ping { msg: "hi".into() }).await.unwrap();
    let reply: String = bus
        .request::<Question, String>(Question { q: "x".into() })
        .await
        .expect("responder replies");
    assert_eq!(reply, "answer:x");
    tokio::time::sleep(Duration::from_millis(150)).await;

    let entries = evidence.snapshot();
    assert!(entries.contains(&"tick:tick".to_string()), "{entries:?}");
    assert!(entries.contains(&"impl:on_ping".to_string()), "{entries:?}");
    assert!(
        entries.contains(&"method:on_ping".to_string()),
        "{entries:?}"
    );
    assert!(entries.contains(&"resp:answer".to_string()), "{entries:?}");
}

#[r2e::test]
async fn pinned_override_bean_is_undecorated() {
    // A pinned bean (`override_bean`) skips registration, so `after_register`
    // never queues the deco fill → the slot stays empty → direct calls run
    // the bare inner (no interceptor).
    let ticks = Arc::new(AtomicUsize::new(0));
    let evidence = Evidence::default();
    let pinned = SchedTicker::new(ticks.clone());
    let app = AppBuilder::new()
        .provide(ticks.clone()) // SchedTicker's constructor dep (type-level)
        .provide(evidence.clone())
        .override_bean(pinned)
        .register::<SchedTicker>() // no-op: pinned
        .build_state()
        .await;

    let bean: SchedTicker = app.bean_context().get();
    bean.tick().await;

    assert_eq!(ticks.load(Ordering::SeqCst), 1, "method body still ran");
    assert!(
        evidence.snapshot().is_empty(),
        "pinned bean is undecorated — interceptor must NOT fire: {:?}",
        evidence.snapshot()
    );
}

#[r2e::test]
async fn direct_call_through_a_clone_held_by_another_bean_is_intercepted() {
    // `Caller` captured a SchedTicker clone during resolution (before the
    // slot fill). Because SharedDecoSlot shares its Arc across clones, the
    // fill is visible through that captured clone.
    let ticks = Arc::new(AtomicUsize::new(0));
    let evidence = Evidence::default();
    let app = AppBuilder::new()
        .provide(ticks.clone())
        .provide(evidence.clone())
        .register::<SchedTicker>()
        .register::<Caller>()
        .build_state()
        .await;

    let caller: Caller = app.bean_context().get();
    caller.poke().await;

    assert_eq!(ticks.load(Ordering::SeqCst), 1);
    assert_eq!(evidence.snapshot(), vec!["tick:tick"]);
}

#[r2e::test]
async fn hand_built_instance_decorated_explicitly() {
    // A hand-built, unregistered instance: `Decorate::decorate` builds its
    // chains from the resolved graph and fills the slot.
    let evidence = Evidence::default();
    let app = AppBuilder::new()
        .provide(evidence.clone())
        .build_state()
        .await;

    let ticks = Arc::new(AtomicUsize::new(0));
    let svc = SchedTicker::new(ticks.clone());
    svc.decorate(app.bean_context());

    svc.tick().await;
    assert_eq!(ticks.load(Ordering::SeqCst), 1);
    assert_eq!(evidence.snapshot(), vec!["tick:tick"]);

    // A clone taken AFTER decorate shares the fill (Arc-backed slot).
    let clone = svc.clone();
    clone.tick().await;
    assert_eq!(evidence.snapshot(), vec!["tick:tick", "tick:tick"]);

    // decorate() is idempotent (OnceLock first-write-wins) — no-op re-run.
    svc.decorate(app.bean_context());
    svc.tick().await;
    assert_eq!(
        evidence.snapshot(),
        vec!["tick:tick", "tick:tick", "tick:tick"]
    );
}

#[r2e::test]
async fn override_bean_decorated_decorates_the_pinned_instance() {
    // `override_bean_decorated` pins the instance AND queues its deco fill, so
    // it is decorated — but its scheduled tasks are still dropped (pin skip).
    let ticks = Arc::new(AtomicUsize::new(0));
    let evidence = Evidence::default();
    let pinned = SchedTicker::new(ticks.clone());
    let app = AppBuilder::new()
        .plugin(Executor)
        .plugin(Scheduler)
        .provide(ticks.clone())
        .provide(evidence.clone())
        .override_bean_decorated(pinned)
        .register::<SchedTicker>() // no-op: pinned
        .register::<Caller>() // injects the pinned SchedTicker
        .build_state()
        .await;

    // The pinned bean pulled from the graph is decorated.
    let bean: SchedTicker = app.bean_context().get();
    bean.tick().await;
    assert_eq!(evidence.snapshot(), vec!["tick:tick"]);

    // And so is the clone Caller captured during resolution (before the fill).
    let caller: Caller = app.bean_context().get();
    caller.poke().await;
    assert_eq!(evidence.snapshot(), vec!["tick:tick", "tick:tick"]);
    assert_eq!(ticks.load(Ordering::SeqCst), 2);

    // Scheduled tasks are STILL dropped (pin skips the scheduled source).
    let registry = app
        .get_plugin_data::<TaskRegistryHandle>()
        .expect("scheduler registry present");
    let tasks = extract_tasks(registry.take_of::<ScheduledTaskMarker>());
    assert!(
        tasks.is_empty(),
        "pinned bean's scheduled tasks must stay dropped, got {}",
        tasks.len()
    );
}
