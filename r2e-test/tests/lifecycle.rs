//! `TestApp` runs the production lifecycle: builder startup hooks and
//! `spawn_service` tasks at boot, and `on_drain` → drain → join → `on_stop`
//! (under both shutdown budgets) on `TestApp::shutdown`.
//!
//! It also runs production's *refusals* (an invalid `server.workers`, a
//! per-worker service with nothing to shard onto), and the two things that
//! separate a test shutdown from a test drop: `shutdown()` takes the signal
//! path and leaves the `StopHandle` unfired, while `Drop` cancels and then
//! aborts — including a task that ignores cancellation.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use r2e_core::rt::CancelToken;
use r2e_core::runtime::service::ServiceComponent;
use r2e_core::type_list::{TCons, TNil};
use r2e_core::{App, AppBuilder, BootableApp, SpawnService};
use r2e_test::TestApp;

/// Ordered log of the lifecycle events an app observes. Provided as a bean so
/// services, hooks and `#[pre_destroy]` disposers all write to the same one.
type Log = Arc<Mutex<Vec<&'static str>>>;

fn events(log: &Log) -> Vec<&'static str> {
    log.lock().unwrap().clone()
}

/// `App::build` takes no test arguments, so the apps below read their log out
/// of a global slot. Tests hold [`TEST_LOCK`] for their whole body — one
/// booting app owns the slot at a time.
static SLOT: Mutex<Option<Log>> = Mutex::new(None);
static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Take the lock, install a fresh log, and hand back both. The guard lives as
/// long as the test binding it.
async fn install_log() -> (tokio::sync::MutexGuard<'static, ()>, Log) {
    let guard = TEST_LOCK.lock().await;
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    *SLOT.lock().unwrap() = Some(Arc::clone(&log));
    (guard, log)
}

/// The installed log, as an app under construction sees it.
fn app_log() -> Log {
    SLOT.lock()
        .unwrap()
        .clone()
        .expect("a test installed a log before booting")
}

/// Poll `cond` until it holds, or fail after `within`. Startup work lands on
/// spawned tasks, so "did the service start?" is inherently a race with the
/// test thread — a bounded wait is the honest form of the assertion.
async fn eventually(within: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    cond()
}

// ── 1. Startup: builder `on_start` hooks and background services ───────────

/// The service `spawn_service` starts — the shape `#[derive(BackgroundService)]`
/// generates. Cooperative: it stops as soon as its token is cancelled.
struct Worker {
    log: Log,
}

impl ServiceComponent for Worker {
    type Deps = TCons<Log, TNil>;

    fn from_context(ctx: &r2e_core::beans::BeanContext) -> Self {
        Self {
            log: ctx.get::<Log>(),
        }
    }

    async fn start(self, shutdown: CancelToken) {
        self.log.lock().unwrap().push("service started");
        shutdown.cancelled().await;
        self.log.lock().unwrap().push("service stopped");
    }
}

/// A bean with a `#[pre_destroy]` disposer, to check the disposers run inside
/// the shutdown sequence (they never fired under the old `TestApp`).
#[derive(Clone)]
struct Resource {
    log: Log,
}

#[r2e_core::prelude::bean]
impl Resource {
    fn new(log: Log) -> Self {
        Self { log }
    }

    #[pre_destroy]
    async fn release(&self) {
        self.log.lock().unwrap().push("pre_destroy");
    }
}

/// The full-lifecycle app: a startup hook, a background service, a
/// `#[pre_destroy]` bean, and drain/stop hooks.
struct LifecycleApp;

impl App for LifecycleApp {
    type Env = ();

    async fn setup() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn build(
        b: AppBuilder,
        _env: (),
    ) -> Result<impl BootableApp, Box<dyn std::error::Error + Send + Sync>> {
        let log = app_log();
        let (on_start, on_drain, on_stop) =
            (Arc::clone(&log), Arc::clone(&log), Arc::clone(&log));
        Ok(b
            .provide(log)
            .register::<Resource>()
            .build_state()
            .await
            .spawn_service::<Worker>()
            .on_start(move |_state| async move {
                on_start.lock().unwrap().push("on_start");
                Ok(())
            })
            .on_drain(move |_state| async move {
                on_drain.lock().unwrap().push("on_drain");
            })
            .on_stop(move |_state| async move {
                on_stop.lock().unwrap().push("on_stop");
            }))
    }
}

#[tokio::test]
async fn boot_runs_startup_hooks_and_starts_services() {
    let (_guard, log) = install_log().await;

    let app = TestApp::boot::<LifecycleApp>().await;

    assert!(
        events(&log).contains(&"on_start"),
        "the builder startup hook must run at boot: {:?}",
        events(&log)
    );
    assert!(
        eventually(Duration::from_secs(2), || events(&log).contains(&"service started")).await,
        "spawn_service must start its task at boot: {:?}",
        events(&log)
    );

    // Nothing shutdown-shaped has run yet.
    assert!(!events(&log).contains(&"on_drain"));
    assert!(!events(&log).contains(&"on_stop"));

    app.shutdown().await;
}

#[tokio::test]
async fn shutdown_runs_drain_then_disposers_then_stop() {
    let (_guard, log) = install_log().await;

    let app = TestApp::boot::<LifecycleApp>().await;
    assert!(eventually(Duration::from_secs(2), || events(&log).contains(&"service started")).await);
    log.lock().unwrap().clear();

    app.shutdown().await;

    let seen = events(&log);
    let index = |needle: &str| {
        seen.iter()
            .position(|e| *e == needle)
            .unwrap_or_else(|| panic!("{needle} never ran: {seen:?}"))
    };
    // Production order: drain hooks (still serving) → async disposers →
    // token cancelled (services stop) → tracked join → on_stop, last of all.
    assert!(index("on_drain") < index("pre_destroy"));
    assert!(index("pre_destroy") < index("service stopped"));
    assert!(index("service stopped") < index("on_stop"));
}

// ── 2. `shutdown_grace_period` bounds the tracked-handle join ──────────────

/// A service that ignores its `CancelToken` — the case the grace period
/// exists for.
struct StuckService {
    log: Log,
}

impl ServiceComponent for StuckService {
    type Deps = TCons<Log, TNil>;

    fn from_context(ctx: &r2e_core::beans::BeanContext) -> Self {
        Self {
            log: ctx.get::<Log>(),
        }
    }

    async fn start(self, _shutdown: CancelToken) {
        self.log.lock().unwrap().push("service started");
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}

struct StuckApp;

impl App for StuckApp {
    type Env = ();

    async fn setup() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn build(
        b: AppBuilder,
        _env: (),
    ) -> Result<impl BootableApp, Box<dyn std::error::Error + Send + Sync>> {
        let log = app_log();
        let on_stop = Arc::clone(&log);
        Ok(b
            .provide(log)
            .build_state()
            .await
            .shutdown_grace_period(Duration::from_millis(300))
            .spawn_service::<StuckService>()
            .on_stop(move |_state| async move {
                on_stop.lock().unwrap().push("on_stop");
            }))
    }
}

#[tokio::test]
async fn shutdown_grace_period_bounds_a_stuck_service() {
    let (_guard, log) = install_log().await;

    let app = TestApp::boot::<StuckApp>().await;
    assert!(eventually(Duration::from_secs(2), || events(&log).contains(&"service started")).await);

    let started = Instant::now();
    app.shutdown().await;
    let elapsed = started.elapsed();

    assert!(
        elapsed >= Duration::from_millis(250),
        "the grace period must actually be waited: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "a service ignoring its token must be abandoned after the grace \
         period, not waited on for its full 30s: {elapsed:?}"
    );
    // `on_stop` is MUST-RUN: it runs after the abandoned join, outside every
    // budget.
    assert_eq!(events(&log).last().copied(), Some("on_stop"));
}

// ── 3. `drain_timeout` bounds the HTTP drain of a live server ──────────────

/// A handler that never finishes within the test's lifetime: the in-flight
/// request the drain budget has to give up on.
async fn slow_handler() -> &'static str {
    tokio::time::sleep(Duration::from_secs(30)).await;
    "too late"
}

struct SlowApp;

impl App for SlowApp {
    type Env = ();

    async fn setup() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn build(
        b: AppBuilder,
        _env: (),
    ) -> Result<impl BootableApp, Box<dyn std::error::Error + Send + Sync>> {
        let log = app_log();
        let on_stop = Arc::clone(&log);
        Ok(b
            .provide(log)
            .build_state()
            .await
            .drain_timeout(Duration::from_millis(300))
            .register_routes(
                r2e_core::http::Router::new()
                    .route("/slow", r2e_core::http::routing::get(slow_handler)),
            )
            .on_stop(move |_state| async move {
                on_stop.lock().unwrap().push("on_stop");
            }))
    }
}

#[tokio::test]
async fn drain_timeout_bounds_the_http_drain() {
    use tokio::io::AsyncWriteExt as _;

    let (_guard, log) = install_log().await;

    let app = TestApp::boot::<SlowApp>().await;
    let server = app.serve().await;

    // Hold one request in flight, without reading the response: the connection
    // stays open for the whole 30s handler, which is what the drain must give
    // up on.
    let mut socket = tokio::net::TcpStream::connect(server.addr())
        .await
        .expect("connect to the test server");
    socket
        .write_all(b"GET /slow HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .expect("send the request");
    tokio::time::sleep(Duration::from_millis(200)).await;

    let started = Instant::now();
    app.shutdown().await;
    let elapsed = started.elapsed();

    assert!(
        elapsed >= Duration::from_millis(250),
        "the drain budget must actually be waited: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "drain_timeout must abandon the in-flight request instead of waiting \
         out the 30s handler: {elapsed:?}"
    );
    assert_eq!(events(&log).last().copied(), Some("on_stop"));
    drop(server);
}

// ── 4. A hook-less app pays nothing ────────────────────────────────────────

struct BareApp;

impl App for BareApp {
    type Env = ();

    async fn setup() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn build(
        b: AppBuilder,
        _env: (),
    ) -> Result<impl BootableApp, Box<dyn std::error::Error + Send + Sync>> {
        Ok(b.build_state().await)
    }
}

#[tokio::test]
async fn an_app_without_hooks_has_no_shutdown_work() {
    let (_guard, log) = install_log().await;

    // A start is not literally free — it allocates the shutdown token, the
    // plugin-hook cell and the handle collector — but it runs no hook and
    // spawns no task, and that is what the predicate reports.
    let bare = TestApp::boot::<BareApp>().await;
    assert!(
        !bare.has_shutdown_work(),
        "no hook, no plugin hook, no tracked task: dropping this app loses \
         nothing a shutdown() would have done"
    );
    bare.shutdown().await;
    assert!(
        events(&log).is_empty(),
        "a hook-less shutdown must run nothing: {:?}",
        events(&log)
    );

    // The contrast that makes the assertion above worth making: the same
    // predicate is `true` as soon as the app owns one live task.
    let with_service = TestApp::boot::<StubbornApp>().await;
    assert!(
        with_service.has_shutdown_work(),
        "a tracked task is shutdown work even when no hook is registered"
    );
    drop(with_service);
}

// ── 5. An in-process start refuses what `run()` refuses ────────────────────
//
// `TestApp::boot` goes through `start_in_process`, which shares `run()`'s
// preconditions. Both of these are boot errors in production; a test boot that
// accepted them would let an app pass its whole suite and then fail to serve.

/// The minimum app that reads config at all — the harness patches
/// `server.workers` into it from the boot hook.
struct ConfiguredApp;

impl App for ConfiguredApp {
    type Env = ();

    async fn setup() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn build(
        b: AppBuilder,
        _env: (),
    ) -> Result<impl BootableApp, Box<dyn std::error::Error + Send + Sync>> {
        Ok(b.load_config::<()>().build_state().await)
    }
}

#[tokio::test]
async fn try_boot_rejects_an_invalid_server_workers() {
    let err = TestApp::try_boot_with::<ConfiguredApp>(|b| b.override_config_value("server.workers", 0i64))
        .await
        .err()
        .expect("`server.workers = 0` must fail the boot, as it fails run()");

    let rendered = err.to_string();
    assert!(
        rendered.contains("server.workers must be a positive integer"),
        "the boot error must be the same one run() reports: {rendered}"
    );
}

/// An app registering a per-worker service — which only sharded serving can
/// start.
struct PerWorkerApp;

impl App for PerWorkerApp {
    type Env = ();

    async fn setup() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn build(
        b: AppBuilder,
        _env: (),
    ) -> Result<impl BootableApp, Box<dyn std::error::Error + Send + Sync>> {
        Ok(b
            .per_worker_service(|_worker| async move { Ok(()) })
            .build_state()
            .await)
    }
}

#[tokio::test]
async fn try_boot_rejects_a_per_worker_service() {
    let err = TestApp::try_boot::<PerWorkerApp>()
        .await
        .err()
        .expect("an in-process start spawns no workers: the service could never run");

    let rendered = err.to_string();
    assert!(
        rendered.contains(r2e_core::builder::PER_WORKER_REQUIRES_SHARDING_MSG),
        "the boot error must name the sharding requirement: {rendered}"
    );
}

// ── 6. Dropping without `shutdown()` stops even an uncooperative task ───────

/// A service that never reads its token — the case where cancelling alone
/// would leave the task running against a released graph.
struct StubbornService {
    log: Log,
}

impl ServiceComponent for StubbornService {
    type Deps = TCons<Log, TNil>;

    fn from_context(ctx: &r2e_core::beans::BeanContext) -> Self {
        Self {
            log: ctx.get::<Log>(),
        }
    }

    async fn start(self, _shutdown: CancelToken) {
        loop {
            self.log.lock().unwrap().push("tick");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

struct StubbornApp;

impl App for StubbornApp {
    type Env = ();

    async fn setup() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn build(
        b: AppBuilder,
        _env: (),
    ) -> Result<impl BootableApp, Box<dyn std::error::Error + Send + Sync>> {
        Ok(b
            .provide(app_log())
            .build_state()
            .await
            .spawn_service::<StubbornService>())
    }
}

#[tokio::test]
async fn dropping_the_app_aborts_a_task_that_ignores_cancellation() {
    let (_guard, log) = install_log().await;

    let app = TestApp::boot::<StubbornApp>().await;
    assert!(eventually(Duration::from_secs(2), || !events(&log).is_empty()).await);
    assert!(app.has_shutdown_work(), "a live service is shutdown work");

    // Control: while the app is alive the service keeps ticking, so a frozen
    // counter after the drop means the drop stopped it — not that it had
    // stopped on its own.
    let before = events(&log).len();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let running = events(&log).len();
    assert!(
        running > before,
        "the service must still be ticking before the drop: {before} → {running}"
    );

    drop(app);
    let at_drop = events(&log).len();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let after = events(&log).len();
    assert!(
        after <= at_drop + 1,
        "dropping the app must abort the task, not detach it — it ticked \
         {} more times in 300ms",
        after - at_drop
    );
}

// ── 7. An early `TestServer` drop drains under `drain_timeout` too ──────────

/// `SlowApp` without the hooks: the tracked server is then the *only* thing
/// `has_shutdown_work()` can be reporting, which is how the test observes the
/// drain finishing.
struct SlowBareApp;

impl App for SlowBareApp {
    type Env = ();

    async fn setup() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn build(
        b: AppBuilder,
        _env: (),
    ) -> Result<impl BootableApp, Box<dyn std::error::Error + Send + Sync>> {
        Ok(b
            .build_state()
            .await
            .drain_timeout(Duration::from_millis(300))
            .register_routes(
                r2e_core::http::Router::new()
                    .route("/slow", r2e_core::http::routing::get(slow_handler)),
            ))
    }
}

#[tokio::test]
async fn dropping_the_server_first_drains_under_drain_timeout() {
    use tokio::io::AsyncWriteExt as _;

    let app = TestApp::boot::<SlowBareApp>().await;
    let server = app.serve().await;
    assert!(
        app.has_shutdown_work(),
        "the attached server is a live tracked handle"
    );

    let mut socket = tokio::net::TcpStream::connect(server.addr())
        .await
        .expect("connect to the test server");
    socket
        .write_all(b"GET /slow HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .expect("send the request");
    tokio::time::sleep(Duration::from_millis(200)).await;

    // The app is NOT cancelled here — only this server stops. Its drain budget
    // must run from its own stop, or the stuck request holds the task for the
    // handler's full 30s.
    let started = Instant::now();
    drop(server);
    let finished = eventually(Duration::from_secs(5), || !app.has_shutdown_work()).await;
    let elapsed = started.elapsed();

    assert!(
        finished,
        "the dropped server's drain must end under drain_timeout, not wait out \
         the 30s handler: {elapsed:?}"
    );
    assert!(
        elapsed >= Duration::from_millis(250),
        "the drain budget must actually be waited: {elapsed:?}"
    );

    app.shutdown().await;
}

// ── 8. `has_shutdown_work()` counts hooks, plugin hooks and live tasks ──────

/// A plugin whose only lifecycle contribution is a *sync* shutdown hook: the
/// third thing `has_shutdown_work()` has to account for, next to user hooks
/// and tracked tasks.
struct SyncHookPlugin {
    log: Log,
}

impl r2e_core::plugin::Plugin for SyncHookPlugin {
    type Provided = ();
    type Deps = ();
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut r2e_core::plugin::PluginBuildContext,
    ) -> Result<(), r2e_core::plugin::PluginBuildError> {
        let log = self.log;
        ctx.on_shutdown(move || {
            log.lock().unwrap().push("plugin sync hook");
        });
        Ok(())
    }
}

struct PluginHookApp;

impl App for PluginHookApp {
    type Env = ();

    async fn setup() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn build(
        b: AppBuilder,
        _env: (),
    ) -> Result<impl BootableApp, Box<dyn std::error::Error + Send + Sync>> {
        Ok(b
            .plugin(SyncHookPlugin { log: app_log() })
            .build_state()
            .await)
    }
}

#[tokio::test]
async fn has_shutdown_work_sees_an_unfired_plugin_sync_hook() {
    let (_guard, log) = install_log().await;

    let app = TestApp::boot::<PluginHookApp>().await;
    assert!(
        app.has_shutdown_work(),
        "a registered plugin sync shutdown hook is work a drop would lose"
    );

    app.shutdown().await;
    assert_eq!(events(&log), vec!["plugin sync hook"]);
}

// ── 9. `shutdown()` is the signal path; `StopHandle` is the other one ───────

/// Where the test parks the app's `StopHandle` so the `on_drain` hook — which
/// only receives the state — can read it back during shutdown.
#[derive(Clone, Default)]
struct StopSlot(Arc<Mutex<Option<r2e_core::StopHandle>>>);

struct StopWatchApp;

impl App for StopWatchApp {
    type Env = ();

    async fn setup() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn build(
        b: AppBuilder,
        _env: (),
    ) -> Result<impl BootableApp, Box<dyn std::error::Error + Send + Sync>> {
        let log = app_log();
        let slot = StopSlot::default();
        let (hook_log, hook_slot) = (Arc::clone(&log), slot.clone());
        Ok(b
            .provide(log)
            .provide(slot)
            .build_state()
            .await
            .on_drain(move |_state| {
                let (log, slot) = (Arc::clone(&hook_log), hook_slot.clone());
                async move {
                    let stopped = slot
                        .0
                        .lock()
                        .unwrap()
                        .as_ref()
                        .expect("the test parks the handle before shutting down")
                        .is_stopped();
                    log.lock()
                        .unwrap()
                        .push(if stopped { "stopped" } else { "not stopped" });
                }
            }))
    }
}

/// Park the app's stop handle where the `on_drain` hook can read it.
fn arm_stop_slot(app: &TestApp) {
    *app.bean::<StopSlot>().0.lock().unwrap() = Some(app.stop_handle());
}

#[tokio::test]
async fn shutdown_leaves_the_stop_handle_unfired_like_sigterm() {
    let (_guard, log) = install_log().await;

    let app = TestApp::boot::<StopWatchApp>().await;
    arm_stop_slot(&app);
    assert!(!app.stop_handle().is_stopped());

    app.shutdown().await;

    // Production's shutdown future is `select!(shutdown_signal(),
    // stop_handle.stopped())`: a SIGTERM never fires the handle, so neither
    // does the default test shutdown.
    assert_eq!(events(&log), vec!["not stopped"]);
}

#[tokio::test]
async fn firing_the_stop_handle_takes_the_programmatic_path() {
    let (_guard, log) = install_log().await;

    let app = TestApp::boot::<StopWatchApp>().await;
    arm_stop_slot(&app);

    app.stop_handle().stop();
    assert!(app.stop_handle().is_stopped());
    app.shutdown().await;

    assert_eq!(events(&log), vec!["stopped"]);
}
