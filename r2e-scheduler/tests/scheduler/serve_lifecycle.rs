//! The scheduler under the *serving* lifecycle: who owns the driver, and what
//! happens to a tick that is still running when the boot aborts.
//!
//! The plugin starts the driver from its serve hook, and serve hooks run BEFORE
//! the fallible startup hooks — so a `.on_start(...)` returning `Err` is the
//! sharpest test of the ownership model: it aborts a boot that already has
//! scheduled work in flight.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use r2e_core::builder::{ScheduledTaskMarker, TaskRegistryHandle};
use r2e_core::AppBuilder;
use r2e_executor::Executor;
use r2e_scheduler::{
    PositiveDuration, ScheduleConfig, ScheduledTask, ScheduledTaskDef, Scheduler,
};

/// A bean the scheduled tick resolves *through the graph*, the way a
/// `#[scheduled]` method on a tenant-aware bean does.
#[derive(Clone)]
struct Alpha(u32);

/// What a tick captures: a weak handle to the graph (this is exactly what a
/// `GraphHandle` holds), plus the places it reports to.
#[derive(Clone)]
struct TickState {
    graph: std::sync::Weak<r2e_core::beans::BeanContext>,
    observations: Arc<Mutex<Vec<String>>>,
    started: Arc<AtomicUsize>,
}

fn boxed_task(task: ScheduledTaskDef<TickState>) -> Box<dyn std::any::Any + Send> {
    let trait_obj: Box<dyn ScheduledTask> = Box::new(task);
    Box::new(trait_obj)
}

/// A slow tick: it starts, sleeps well past the moment the boot aborts, and
/// only then resolves the bean — so what it observes is the state of the graph
/// *after* `run_inner` decided to give up.
fn slow_probing_task(state: TickState) -> ScheduledTaskDef<TickState> {
    ScheduledTaskDef {
        overlap: r2e_scheduler::OverlapPolicy::Skip,
        skip: None,
        name: "probe".to_string(),
        schedule: ScheduleConfig::Interval(PositiveDuration::from_millis(20).unwrap()),
        state,
        task: Box::new(|s| {
            Box::pin(async move {
                s.started.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(300)).await;
                let observed = match s.graph.upgrade().and_then(|g| g.try_get::<Alpha>()) {
                    Some(Alpha(v)) => format!("alive-{v}"),
                    None => "gone".to_string(),
                };
                s.observations.lock().unwrap().push(observed);
            })
        }),
    }
}

#[tokio::test]
async fn an_aborted_boot_winds_the_scheduler_down_before_run_returns() {
    let observations = Arc::new(Mutex::new(Vec::new()));
    let started = Arc::new(AtomicUsize::new(0));

    let app = AppBuilder::new()
        .plugin(Scheduler)
        .plugin(Executor)
        .provide(Alpha(21))
        .build_state()
        .await;

    let state = TickState {
        graph: Arc::downgrade(app.bean_context()),
        observations: observations.clone(),
        started: started.clone(),
    };
    app.get_plugin_data::<TaskRegistryHandle>()
        .expect("the Scheduler plugin stores a task registry")
        .add_boxed_for::<ScheduledTaskMarker>(vec![boxed_task(slow_probing_task(state))]);

    // Give the first tick time to fire (20ms cadence) and be in flight, then
    // abort the boot.
    let app = app.on_start(|_state| async {
        tokio::time::sleep(Duration::from_millis(120)).await;
        Err::<(), Box<dyn std::error::Error + Send + Sync>>("startup hook says no".into())
    });

    let weak = Arc::downgrade(app.bean_context());
    let prepared = app.prepare("127.0.0.1:0");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let err = tokio::time::timeout(Duration::from_secs(10), prepared.run_with_listener(listener))
        .await
        .expect("run() must return: the aborted boot cancels the scheduler")
        .expect_err("the failing startup hook must abort the boot");
    assert!(
        err.to_string().contains("startup hook says no"),
        "unexpected error: {err}"
    );

    // Read the observations with NO sleep in between: everything the scheduler
    // still had running must be finished by the time `run()` returns, because
    // the driver is a tracked task (it owns the graph and is joined on the
    // abort path) and it waits out its in-flight ticks before completing.
    assert!(
        started.load(Ordering::SeqCst) >= 1,
        "the test is meaningless unless a tick was in flight when the boot aborted"
    );
    let observed = observations.lock().unwrap().clone();
    assert_eq!(
        observed.len(),
        started.load(Ordering::SeqCst),
        "every tick that started must have finished before run() returned, got {observed:?}"
    );
    assert!(
        observed.iter().all(|o| o == "alive-21"),
        "a tick running through an aborted boot must still resolve its beans \
         through the graph, got {observed:?}"
    );

    // …and once it is over, the scheduler holds nothing: no driver task, no
    // graph.
    assert!(
        weak.upgrade().is_none(),
        "the wound-down scheduler must not pin the graph"
    );
}
