//! [`SharedEnv`]: one `App::Env` per test binary, built on a runtime that
//! outlives every test (Tasker #988, review round 1).
//!
//! The failure this file exists to catch is a *lifetime* bug, not a value bug.
//! Memoising the environment in a bare `OnceCell` stores the right value, but
//! runs `setup()` on whichever **per-test** runtime won the race — and that
//! runtime is dropped when its test returns. Everything the environment bound
//! to it (listeners, pool keep-alive tasks, timers, anything `setup` spawned)
//! dies with it, silently, and later tests hang on an inert environment.
//!
//! So every app below parks a **runtime-bound** resource in `setup()`: a task
//! that answers pings. A dead reactor is then observable as "no answer", not as
//! a wrong value.
//!
//! `#[r2e::test]` / `#[r2e::test_suite]` resolve `r2e-test` as `crate::` inside
//! this package (proc-macro-crate reports `FoundCrate::Itself`), hence the
//! re-exports at the crate root.

pub use r2e_test::{ordering, suite, TestApp};

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use r2e_core::rt::sync::{mpsc, oneshot};
use r2e_core::rt::{self, RuntimeBuilder};
use r2e_core::{App, AppBuilder, BootableApp};
use r2e_test::SharedEnv;

// ---------------------------------------------------------------------------
// A runtime-bound environment: a task spawned by `setup()`.
// ---------------------------------------------------------------------------

/// A handle onto a task `setup()` spawned — the thing that dies with the
/// runtime that created it.
#[derive(Clone, Debug)]
struct Heartbeat {
    pings: mpsc::Sender<oneshot::Sender<u64>>,
}

impl Heartbeat {
    /// Ask the background task for the next tick.
    ///
    /// `None` = the task is gone (its runtime was dropped): the exact failure
    /// this module guards against. Bounded, so a dead reactor fails the test
    /// instead of hanging it.
    async fn ping(&self) -> Option<u64> {
        rt::timeout(Duration::from_secs(5), async {
            let (reply, answer) = oneshot::channel();
            self.pings.send(reply).await.ok()?;
            answer.await.ok()
        })
        .await
        .ok()
        .flatten()
    }
}

/// Spawn the heartbeat task **on the ambient runtime** — which is the whole
/// point: whichever runtime is current when `setup()` runs owns it.
fn spawn_heartbeat() -> Heartbeat {
    let (pings, mut rx) = mpsc::channel::<oneshot::Sender<u64>>(16);
    rt::spawn(async move {
        let mut ticks = 0u64;
        while let Some(reply) = rx.recv().await {
            ticks += 1;
            let _ = reply.send(ticks);
        }
    });
    Heartbeat { pings }
}

/// Declare a tiny `App` whose `Env` is a [`Heartbeat`], plus the counter its
/// `setup()` bumps. The apps differ only by identity so that each test group
/// owns its own environment (and its own setup count).
macro_rules! heartbeat_app {
    ($app:ident, $setups:ident) => {
        static $setups: AtomicUsize = AtomicUsize::new(0);

        struct $app;

        impl App for $app {
            type Env = Heartbeat;

            async fn setup() -> Result<Heartbeat, Box<dyn std::error::Error + Send + Sync>> {
                $setups.fetch_add(1, Ordering::SeqCst);
                Ok(spawn_heartbeat())
            }

            async fn build(
                b: AppBuilder,
                env: Heartbeat,
            ) -> Result<impl BootableApp, Box<dyn std::error::Error + Send + Sync>> {
                Ok(b.provide(env).build_state().await)
            }
        }
    };
}

// ---------------------------------------------------------------------------
// 1. The regression: the environment must outlive the runtime that built it.
// ---------------------------------------------------------------------------

heartbeat_app!(DeathApp, DEATH_SETUPS);

static DEATH_ENV: SharedEnv<DeathApp> = SharedEnv::new();

/// Reproduces, deterministically, what `#[r2e::test]` does across two tests:
/// runtime A asks for the environment first and is then dropped; runtime B
/// (a later test) boots on the same environment.
///
/// With a bare `OnceCell` the heartbeat task belongs to runtime A and the
/// second half times out. With [`SharedEnv`] `setup()` never runs on either
/// runtime, so the task survives.
#[test]
fn shared_env_outlives_the_runtime_that_built_it() {
    let first = RuntimeBuilder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime A");
    let env = first.block_on(async { DEATH_ENV.get().await });
    assert_eq!(
        first.block_on(env.ping()),
        Some(1),
        "the environment must answer while the runtime that asked for it is alive"
    );
    // Exactly what happens at the end of a `#[r2e::test]`.
    drop(first);

    let second = RuntimeBuilder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime B");
    second.block_on(async {
        let app = TestApp::boot_env::<DeathApp>(DEATH_ENV.get().await).await;
        assert_eq!(
            app.bean::<Heartbeat>().ping().await,
            Some(2),
            "the task `setup()` spawned must survive the runtime that first \
             asked for the environment"
        );
        app.shutdown().await;
    });
    assert_eq!(DEATH_SETUPS.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// 2. The same thing through the macro knob, on two tests forced to run in
//    sequence: the second one runs after the first one's runtime is gone.
// ---------------------------------------------------------------------------

heartbeat_app!(PingApp, PING_SETUPS);

static PING_ENV: SharedEnv<PingApp> = SharedEnv::new();

#[r2e::test(app = PingApp, env = PING_ENV.get().await, group = "shared-env-ping", order = 1)]
async fn the_first_test_gets_a_live_environment(app: TestApp) {
    assert!(app.bean::<Heartbeat>().ping().await.is_some());
    assert_eq!(PING_SETUPS.load(Ordering::SeqCst), 1);
}

#[r2e::test(app = PingApp, env = PING_ENV.get().await, group = "shared-env-ping", order = 2)]
async fn a_later_test_gets_the_same_live_environment(app: TestApp) {
    assert!(
        app.bean::<Heartbeat>().ping().await.is_some(),
        "the environment must still be driven after the previous test's runtime was dropped"
    );
    assert_eq!(
        PING_SETUPS.load(Ordering::SeqCst),
        1,
        "`setup()` runs once per binary, not once per test"
    );
}

// ---------------------------------------------------------------------------
// 3. One setup for a whole suite *and* the standalone tests in the same binary.
// ---------------------------------------------------------------------------

struct PingSuite {
    beat: Heartbeat,
    seen: usize,
}

#[r2e::test_suite(app = PingApp, env = PING_ENV.get().await, tracing = false)]
impl PingSuite {
    #[before_all]
    async fn boot(app: TestApp) -> Self {
        assert_eq!(
            PING_SETUPS.load(Ordering::SeqCst),
            1,
            "the suite must reuse the binary's environment, not build its own"
        );
        Self {
            beat: app.bean::<Heartbeat>(),
            seen: 0,
        }
    }

    #[case]
    async fn first_case_reaches_the_shared_environment(&mut self) {
        assert!(self.beat.ping().await.is_some());
        self.seen += 1;
    }

    #[case]
    async fn second_case_reaches_it_too(&mut self) {
        assert!(self.beat.ping().await.is_some());
        self.seen += 1;
    }

    #[after_all]
    async fn teardown(&mut self) {
        assert_eq!(self.seen, 2);
        // The suite runtime is about to be shut down; the environment is not.
        assert!(self.beat.ping().await.is_some());
        assert_eq!(PING_SETUPS.load(Ordering::SeqCst), 1);
    }
}

// ---------------------------------------------------------------------------
// 4. Single flight: concurrent first callers share one `setup()` run.
// ---------------------------------------------------------------------------

/// A slow environment, so the racers really are concurrent, carrying the
/// generation of the `setup()` call that produced it.
#[derive(Clone, Debug, PartialEq)]
struct SlowEnv {
    generation: usize,
}

static SLOW_SETUPS: AtomicUsize = AtomicUsize::new(0);

struct SlowApp;

impl App for SlowApp {
    type Env = SlowEnv;

    async fn setup() -> Result<SlowEnv, Box<dyn std::error::Error + Send + Sync>> {
        let generation = SLOW_SETUPS.fetch_add(1, Ordering::SeqCst) + 1;
        rt::sleep(Duration::from_millis(150)).await;
        Ok(SlowEnv { generation })
    }

    async fn build(
        b: AppBuilder,
        env: SlowEnv,
    ) -> Result<impl BootableApp, Box<dyn std::error::Error + Send + Sync>> {
        Ok(b.provide(env).build_state().await)
    }
}

static SLOW_ENV: SharedEnv<SlowApp> = SharedEnv::new();

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_first_callers_share_one_setup() {
    let racers: Vec<_> = (0..16)
        .map(|_| rt::spawn(async { SLOW_ENV.get().await }))
        .collect();

    let mut envs = Vec::new();
    for racer in racers {
        envs.push(racer.await.expect("racer"));
    }

    assert_eq!(
        SLOW_SETUPS.load(Ordering::SeqCst),
        1,
        "16 concurrent first callers must produce exactly one `setup()` run"
    );
    assert!(
        envs.iter().all(|env| env == &SlowEnv { generation: 1 }),
        "every caller must get the one environment: {envs:?}"
    );
    // A late caller takes the memoised value, still without a second setup.
    assert_eq!(SLOW_ENV.get().await, SlowEnv { generation: 1 });
    assert_eq!(SLOW_SETUPS.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// 4b. Publication does not depend on someone still waiting.
// ---------------------------------------------------------------------------

heartbeat_app!(LateApp, LATE_SETUPS);

static LATE_ENV: SharedEnv<LateApp> = SharedEnv::new();

/// The first caller starts `setup` and then walks away (its test was
/// cancelled, or it simply had not reached the wait yet). The environment must
/// still be stored for the next caller.
///
/// This is a real trap: `watch::Sender::send` *refuses and discards* the value
/// when the receiver count is zero, which would leave the slot empty for good
/// and hang every later caller.
#[tokio::test]
async fn an_environment_published_with_nobody_waiting_is_still_delivered() {
    // One poll: enough to start the run, not enough to wait for it. Dropping
    // the future then leaves the publishing thread with zero receivers.
    // (Not asserted: on a loaded machine the run can finish inside that single
    // poll, which only makes the test weaker, never wrong.)
    let _walked_away = rt::timeout(Duration::ZERO, LATE_ENV.get()).await;

    rt::sleep(Duration::from_millis(200)).await;

    let env = rt::timeout(Duration::from_secs(5), LATE_ENV.get())
        .await
        .expect("a value published while nobody waited must still be delivered");
    assert!(env.ping().await.is_some());
    assert_eq!(LATE_SETUPS.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// 5. A failed environment stays failed — and is reported, not retried.
// ---------------------------------------------------------------------------

static FAIL_SETUPS: AtomicUsize = AtomicUsize::new(0);

struct FailApp;

impl App for FailApp {
    type Env = ();

    async fn setup() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        FAIL_SETUPS.fetch_add(1, Ordering::SeqCst);
        Err("the container never came up".into())
    }

    async fn build(
        b: AppBuilder,
        _env: (),
    ) -> Result<impl BootableApp, Box<dyn std::error::Error + Send + Sync>> {
        Ok(b.build_state().await)
    }
}

static FAIL_ENV: SharedEnv<FailApp> = SharedEnv::new();

#[tokio::test]
async fn a_failed_environment_is_reported_once_and_remembered() {
    let first = FAIL_ENV.try_get().await.expect_err("setup fails");
    assert!(
        first.chain().contains("the container never came up"),
        "unexpected error: {first}"
    );
    assert!(first.app().contains("FailApp"));
    assert!(first.to_string().contains("SharedEnv"));

    let second = FAIL_ENV.try_get().await.expect_err("still failed");
    assert_eq!(second.chain(), first.chain());
    assert_eq!(
        FAIL_SETUPS.load(Ordering::SeqCst),
        1,
        "a failed `setup()` must not be retried: its side effects already happened"
    );
}

// ---------------------------------------------------------------------------
// 6. `Env = ()`: every `_env` form stays ergonomic for an app with no
//    environment, and a custom initializer composes setup + seeding.
// ---------------------------------------------------------------------------

static UNIT_SETUPS: AtomicUsize = AtomicUsize::new(0);
static UNIT_SEEDS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Debug, PartialEq)]
struct Marker(&'static str);

struct UnitApp;

impl App for UnitApp {
    type Env = ();

    async fn setup() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        UNIT_SETUPS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn build(
        b: AppBuilder,
        _env: (),
    ) -> Result<impl BootableApp, Box<dyn std::error::Error + Send + Sync>> {
        Ok(b.provide(Marker("real")).build_state().await)
    }
}

/// The "setup, then seed once" form.
static UNIT_ENV: SharedEnv<UnitApp> = SharedEnv::with(|| {
    Box::pin(async {
        UnitApp::setup().await?;
        UNIT_SEEDS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
});

#[tokio::test]
async fn unit_environments_work_through_every_env_boot() {
    let env = UNIT_ENV.get().await;

    let plain = TestApp::boot_env::<UnitApp>(env).await;
    assert_eq!(plain.bean::<Marker>(), Marker("real"));
    plain.shutdown().await;

    let configured =
        TestApp::boot_with_env::<UnitApp>((), |b| b.override_bean(Marker("mock"))).await;
    assert_eq!(configured.bean::<Marker>(), Marker("mock"));
    configured.shutdown().await;

    let plain_boot = TestApp::boot_plain_env::<UnitApp>((), |b| b).await;
    plain_boot.shutdown().await;

    let tried = TestApp::try_boot_env::<UnitApp>(())
        .await
        .expect("boot succeeds");
    tried.shutdown().await;

    let tried_with = TestApp::try_boot_with_env::<UnitApp>((), |b| b)
        .await
        .expect("boot succeeds");
    tried_with.shutdown().await;

    let tried_plain = TestApp::try_boot_plain_env::<UnitApp>((), |b| b)
        .await
        .expect("boot succeeds");
    tried_plain.shutdown().await;

    // The custom initializer ran once; the six boots added nothing.
    assert_eq!(UNIT_SETUPS.load(Ordering::SeqCst), 1);
    assert_eq!(UNIT_SEEDS.load(Ordering::SeqCst), 1);
}

#[r2e::test(app = UnitApp, env = UNIT_ENV.get().await)]
async fn the_macro_knob_accepts_a_unit_environment(app: TestApp) {
    assert_eq!(app.bean::<Marker>(), Marker("real"));
    assert_eq!(UNIT_SETUPS.load(Ordering::SeqCst), 1);
}
