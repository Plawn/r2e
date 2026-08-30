//! `TestApp::boot::<A>()` / `boot_with`: booting an `App`, pinned mocks,
//! config overrides, bean access.

use r2e_core::config::{ConfigValue, R2eConfig};
use r2e_core::{App, AppBuilder, BootableApp};
// `pub`, not a plain `use`: `#[r2e::test(app = …)]` resolves `r2e-test` as
// `crate::` inside this package (proc-macro-crate reports `FoundCrate::Itself`),
// so the generated code looks for `crate::TestApp` in *this* test binary.
pub use r2e_test::TestApp;

#[derive(Clone, Debug, PartialEq)]
struct Greeter {
    origin: &'static str,
}

/// A minimal `App`, shaped like a real app's `lib.rs` declaration.
struct DemoApp;

impl App for DemoApp {
    type Env = ();

    async fn setup() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn build(
        b: AppBuilder,
        _env: (),
    ) -> Result<impl BootableApp, Box<dyn std::error::Error + Send + Sync>> {
        Ok({
            let mut config = R2eConfig::empty();
            config.set("app.greeting", ConfigValue::String("prod".into()));
            b.override_config(config)
                .load_config::<()>()
                .provide(Greeter { origin: "real" })
                .build_state()
                .await
        })
    }
}

/// An `App` that records the active profile it was built under.
struct ProfileApp;

impl App for ProfileApp {
    type Env = ();

    async fn setup() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn build(
        b: AppBuilder,
        _env: (),
    ) -> Result<impl BootableApp, Box<dyn std::error::Error + Send + Sync>> {
        Ok({
            let profile = b.active_profile().to_string();
            b.provide(profile).build_state().await
        })
    }
}

#[tokio::test]
async fn boot_exposes_beans_and_config() {
    let app = TestApp::boot::<DemoApp>().await;

    assert_eq!(app.bean::<Greeter>(), Greeter { origin: "real" });
    assert_eq!(app.config().get::<String>("app.greeting").unwrap(), "prod");
}

#[tokio::test]
async fn boot_with_pins_mocks_over_app_beans() {
    let app = TestApp::boot_with::<DemoApp>(|b| b.override_bean(Greeter { origin: "mock" })).await;

    assert_eq!(app.bean::<Greeter>(), Greeter { origin: "mock" });
}

#[tokio::test]
async fn boot_with_patches_config_keys() {
    let app =
        TestApp::boot_with::<DemoApp>(|b| b.override_config_value("app.greeting", "patched")).await;

    assert_eq!(
        app.config().get::<String>("app.greeting").unwrap(),
        "patched"
    );
}

#[tokio::test]
async fn boot_forces_test_profile() {
    let app = TestApp::boot::<ProfileApp>().await;
    assert_eq!(app.bean::<String>(), "test");
}

#[tokio::test]
async fn boot_wires_a_test_jwt() {
    let app = TestApp::boot::<DemoApp>().await;
    // The TestJwt is available and mints parseable tokens.
    let token = app.test_jwt().token("alice", &["admin"]);
    assert_eq!(token.matches('.').count(), 2, "expected a JWT-shaped token");
}

// ── `#[on_start]` under TestApp::boot ───────────────────────────────────

type StartLog = std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>;

/// A bean with a startup observer. `#[on_start]` runs on the boot path (there
/// IS a startup under `TestApp::boot`); `#[pre_destroy]` needs a shutdown, so
/// it fires on `TestApp::shutdown` — see `tests/lifecycle.rs`.
#[derive(Clone)]
struct Warmer {
    log: StartLog,
}

#[r2e_core::prelude::bean]
impl Warmer {
    fn new(log: StartLog) -> Self {
        Self { log }
    }

    #[on_start]
    async fn warm(&self) {
        self.log.lock().unwrap().push("warmed");
    }
}

static BOOT_LOG: std::sync::OnceLock<StartLog> = std::sync::OnceLock::new();

struct WarmApp;

impl App for WarmApp {
    type Env = ();

    async fn setup() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn build(
        b: AppBuilder,
        _env: (),
    ) -> Result<impl BootableApp, Box<dyn std::error::Error + Send + Sync>> {
        Ok({
            let log = std::sync::Arc::clone(BOOT_LOG.get().expect("BOOT_LOG set by the test"));
            b.provide(log).register::<Warmer>().build_state().await
        })
    }
}

#[tokio::test]
async fn boot_runs_on_start_hooks() {
    let log: StartLog = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    BOOT_LOG.set(std::sync::Arc::clone(&log)).ok();

    let _app = TestApp::boot::<WarmApp>().await;

    assert_eq!(*log.lock().unwrap(), vec!["warmed"]);
}

// ── Fallible boot ──────────────────────────────────────────────────────────

/// A boot error carrying a `source()`, the shape a real driver error has.
#[derive(Debug)]
struct BootFailure(&'static str);

impl std::fmt::Display for BootFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "boot failed: {}", self.0)
    }
}

impl std::error::Error for BootFailure {}

/// `setup` is the phase apps used to end with `process::exit(1)`, which killed
/// the whole test binary. It returns an error now.
struct SetupFailsApp;

impl App for SetupFailsApp {
    type Env = ();

    async fn setup() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Err(Box::new(BootFailure("DATABASE_URL points nowhere")))
    }

    async fn build(
        b: AppBuilder,
        _env: (),
    ) -> Result<impl BootableApp, Box<dyn std::error::Error + Send + Sync>> {
        Ok(b.build_state().await)
    }
}

/// A bean that refuses to build — the `build`-phase counterpart.
#[derive(Clone)]
struct Locked;

#[r2e_core::prelude::bean]
impl Locked {
    fn new() -> Result<Self, BootFailure> {
        Err(BootFailure("advisory lock already held"))
    }
}

struct BuildFailsApp;

impl App for BuildFailsApp {
    type Env = ();

    async fn setup() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn build(
        b: AppBuilder,
        _env: (),
    ) -> Result<impl BootableApp, Box<dyn std::error::Error + Send + Sync>> {
        Ok(b.register::<Locked>().try_build_state().await?)
    }
}

#[tokio::test]
async fn try_boot_surfaces_a_setup_failure() {
    let err = TestApp::try_boot::<SetupFailsApp>()
        .await
        .map(|_| ())
        .expect_err("setup fails");

    assert!(
        err.to_string().contains("DATABASE_URL points nowhere"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn try_boot_surfaces_a_bean_failure() {
    let err = TestApp::try_boot::<BuildFailsApp>()
        .await
        .map(|_| ())
        .expect_err("the graph refuses to resolve");

    let rendered = err.to_string();
    assert!(rendered.contains("Locked"), "unexpected error: {rendered}");
    assert!(
        rendered.contains("advisory lock already held"),
        "unexpected error: {rendered}"
    );
}

/// The panicking form must fail ONE test with an attributable message — the
/// behaviour a `process::exit` in `setup` destroys, since that takes down the
/// whole binary with no failure attributed to any test.
#[test]
fn boot_turns_a_failure_into_an_attributable_test_failure() {
    // Its own thread + runtime: the panic has to be caught, and the boot
    // future is not `Send` (it holds the builder across awaits).
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {})); // keep the expected panic quiet
    let joined = std::thread::spawn(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        rt.block_on(async {
            TestApp::boot::<SetupFailsApp>().await;
        });
    })
    .join();
    std::panic::set_hook(previous);

    let panic = joined.expect_err("boot panics");
    let message = panic
        .downcast_ref::<String>()
        .cloned()
        .expect("expected a string panic payload");

    assert!(
        message.contains("SetupFailsApp"),
        "the panic must name the app: {message}"
    );
    assert!(
        message.contains("DATABASE_URL points nowhere"),
        "the panic must name the cause: {message}"
    );
}

// ── Fallible startup hooks ────────────────────────────────────────────────
//
// `TestApp` boots through `into_router_with_consumers`, which also runs the
// controller `#[post_construct]` and `#[on_start]` hooks. A hook that fails is
// a boot failure like any other: it must reach `try_boot` as an `Err` and be
// rendered by `boot` with the app name — not panic underneath the harness.

use r2e_core::prelude::{controller, routes};
use r2e_core::RegisterControllers;

#[controller]
struct WarmCacheController;

#[routes]
impl WarmCacheController {
    #[get("/warm")]
    async fn warm(&self) -> String {
        "warm".to_string()
    }

    #[post_construct]
    async fn prime(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Err(Box::new(BootFailure("cache priming failed")))
    }
}

struct PostConstructFailsApp;

impl App for PostConstructFailsApp {
    type Env = ();

    async fn setup() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn build(
        b: AppBuilder,
        _env: (),
    ) -> Result<impl BootableApp, Box<dyn std::error::Error + Send + Sync>> {
        Ok(b.try_build_state()
            .await?
            .register_controllers::<(WarmCacheController,)>())
    }
}

#[controller]
struct AnnounceController;

#[routes]
impl AnnounceController {
    #[get("/announce")]
    async fn announce(&self) -> String {
        "announced".to_string()
    }

    #[on_start]
    async fn register_with_discovery(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    {
        Err(Box::new(BootFailure("service discovery refused")))
    }
}

struct OnStartFailsApp;

impl App for OnStartFailsApp {
    type Env = ();

    async fn setup() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn build(
        b: AppBuilder,
        _env: (),
    ) -> Result<impl BootableApp, Box<dyn std::error::Error + Send + Sync>> {
        Ok(b.try_build_state()
            .await?
            .register_controllers::<(AnnounceController,)>())
    }
}

#[tokio::test]
async fn try_boot_surfaces_a_controller_post_construct_failure() {
    let err = TestApp::try_boot::<PostConstructFailsApp>()
        .await
        .map(|_| ())
        .expect_err("the post-construct hook fails");

    let rendered = err.to_string();
    assert!(
        rendered.contains("cache priming failed"),
        "unexpected error: {rendered}"
    );
    assert!(
        rendered.contains("post_construct"),
        "the error must say which lifecycle phase failed: {rendered}"
    );
}

#[tokio::test]
async fn try_boot_surfaces_a_controller_on_start_failure() {
    let err = TestApp::try_boot::<OnStartFailsApp>()
        .await
        .map(|_| ())
        .expect_err("the on_start hook fails");

    let rendered = err.to_string();
    assert!(
        rendered.contains("service discovery refused"),
        "unexpected error: {rendered}"
    );
    assert!(
        rendered.contains("on_start"),
        "the error must say which lifecycle phase failed: {rendered}"
    );
}

/// The panicking form must render a startup-hook failure exactly like any
/// other boot failure: the app name plus the cause, attributed to this test.
#[test]
fn boot_renders_a_startup_hook_failure_with_the_app_name() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let joined = std::thread::spawn(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        rt.block_on(async {
            TestApp::boot::<OnStartFailsApp>().await;
        });
    })
    .join();
    std::panic::set_hook(previous);

    let panic = joined.expect_err("boot panics");
    let message = panic
        .downcast_ref::<String>()
        .cloned()
        .expect("expected a string panic payload");

    assert!(
        message.contains("OnStartFailsApp"),
        "the panic must name the app: {message}"
    );
    assert!(
        message.contains("service discovery refused"),
        "the panic must name the cause: {message}"
    );
}

// ── Config failures ───────────────────────────────────────────────────────
//
// `load_config()` is a type-state transition in the middle of the builder
// chain, so it cannot return a `Result`. It records the failure instead, and
// `try_build_state()` reports it before a single bean is built — which is what
// makes a bad config an `Err` from `try_boot` rather than a panic under the
// harness.

/// Asks for a config file that is not there. `with_config_file` is strict by
/// contract: an explicitly requested file must exist.
struct MissingConfigFileApp;

impl App for MissingConfigFileApp {
    type Env = ();

    async fn setup() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn build(
        b: AppBuilder,
        _env: (),
    ) -> Result<impl BootableApp, Box<dyn std::error::Error + Send + Sync>> {
        Ok(b.with_config_file("does-not-exist-984.yaml")
            .load_config::<()>()
            .provide(Greeter { origin: "never built" })
            .try_build_state()
            .await?)
    }
}

#[tokio::test]
async fn try_boot_surfaces_a_missing_config_file() {
    let err = TestApp::try_boot::<MissingConfigFileApp>()
        .await
        .map(|_| ())
        .expect_err("the requested config file is absent");

    let rendered = err.to_string();
    assert!(
        rendered.contains("Failed to load config"),
        "unexpected error: {rendered}"
    );
    assert!(
        rendered.contains("does-not-exist-984.yaml"),
        "the error must name the file it could not read: {rendered}"
    );
    // The underlying io/parse error stays reachable, so `exit_on_boot_error`
    // can print it as a `caused by:` line.
    assert!(
        std::error::Error::source(&*err).is_some(),
        "the cause chain must survive: {rendered}"
    );
}

#[tokio::test]
async fn a_recorded_config_failure_aborts_before_any_bean_is_built() {
    // The app provides a bean after `load_config`. `try_build_state()` returns
    // the recorded config error first, so the graph is never resolved.
    let err = TestApp::try_boot::<MissingConfigFileApp>()
        .await
        .map(|_| ())
        .expect_err("boot fails");

    assert!(
        !err.to_string().contains("Greeter"),
        "the failure must be the config one, reported before bean resolution: {err}"
    );
}

// ── Sharing one `App::Env` across boots (#988) ────────────────────────────
//
// `App::Env` is the production concept of "resources built once". The `*_env`
// boots hand an environment the caller already owns straight to `App::build`,
// so a test binary can build it once (a `OnceCell`/`LazyLock`) instead of
// replaying pools and migrations per test.

use std::sync::atomic::{AtomicUsize, Ordering};
use r2e_core::rt::sync::OnceCell;

/// Stands in for the expensive thing a real `setup()` builds — a pool, a
/// migrated schema, a container. Its `generation` witnesses *which* setup call
/// produced it.
#[derive(Clone, Debug, PartialEq)]
struct SharedPool {
    generation: usize,
}

/// How many times `EnvApp::setup()` ran in this process. The whole point of the
/// `*_env` boots is that `TestApp` never adds to it.
static ENV_SETUPS: AtomicUsize = AtomicUsize::new(0);

struct EnvApp;

impl App for EnvApp {
    type Env = SharedPool;

    async fn setup() -> Result<SharedPool, Box<dyn std::error::Error + Send + Sync>> {
        Ok(SharedPool {
            generation: ENV_SETUPS.fetch_add(1, Ordering::SeqCst) + 1,
        })
    }

    async fn build(
        b: AppBuilder,
        env: SharedPool,
    ) -> Result<impl BootableApp, Box<dyn std::error::Error + Send + Sync>> {
        Ok(b.provide(env)
            .provide(Greeter { origin: "real" })
            .build_state()
            .await
            .on_stop(|_state| async {
                ENV_STOPS.fetch_add(1, Ordering::SeqCst);
            }))
    }
}

/// The same app **without** the `on_stop` hook, for the tests that must not
/// disturb [`ENV_STOPS`] (it is a process-wide counter and the runner
/// interleaves tests). Its `setup` delegates, so any accidental `A::setup()`
/// call from the harness still moves [`ENV_SETUPS`].
struct PlainEnvApp;

impl App for PlainEnvApp {
    type Env = SharedPool;

    async fn setup() -> Result<SharedPool, Box<dyn std::error::Error + Send + Sync>> {
        EnvApp::setup().await
    }

    async fn build(
        b: AppBuilder,
        env: SharedPool,
    ) -> Result<impl BootableApp, Box<dyn std::error::Error + Send + Sync>> {
        Ok(b.provide(env)
            .provide(Greeter { origin: "real" })
            .build_state()
            .await)
    }
}

/// How many `EnvApp` `on_stop` hooks fired — proof a booted app still owns a
/// full lifecycle even though its environment is borrowed from the binary.
/// Only `boot_env_reuses_one_environment_across_boots` boots `EnvApp`, so the
/// count is its own.
static ENV_STOPS: AtomicUsize = AtomicUsize::new(0);

/// The binary-wide environment: `EnvApp::setup()` runs at most once here, and
/// every boot below reuses the value.
static SHARED_ENV: OnceCell<SharedPool> = OnceCell::const_new();

async fn shared_env() -> SharedPool {
    SHARED_ENV
        .get_or_init(|| async { EnvApp::setup().await.expect("setup") })
        .await
        .clone()
}

#[tokio::test]
async fn boot_env_reuses_one_environment_across_boots() {
    let env = shared_env().await;
    // The only `setup()` this test tolerates is the `OnceCell`'s own.
    let setups = ENV_SETUPS.load(Ordering::SeqCst);

    let first = TestApp::boot_env::<EnvApp>(env.clone()).await;
    let second = TestApp::boot_with_env::<EnvApp>(env.clone(), |b| {
        b.override_bean(Greeter { origin: "mock" })
    })
    .await;

    // `TestApp` called `setup()` zero times: the count did not move.
    assert_eq!(
        ENV_SETUPS.load(Ordering::SeqCst),
        setups,
        "boot_env / boot_with_env must not call A::setup()"
    );
    // The environment passed in is the one `A::build` received — same value,
    // same generation, for both boots.
    assert_eq!(first.bean::<SharedPool>(), env);
    assert_eq!(second.bean::<SharedPool>(), env);
    // The harness defaults still apply, and the `configure` hook still runs.
    assert_eq!(first.bean::<Greeter>(), Greeter { origin: "real" });
    assert_eq!(second.bean::<Greeter>(), Greeter { origin: "mock" });
    assert_eq!(first.test_jwt().token("alice", &["admin"]).matches('.').count(), 2);

    // Lifecycle is untouched: each app shuts down on its own.
    first.shutdown().await;
    second.shutdown().await;
    assert_eq!(
        ENV_STOPS.load(Ordering::SeqCst),
        2,
        "each booted app must still run its own shutdown sequence"
    );
}

#[tokio::test]
async fn boot_plain_env_skips_setup_too() {
    let env = shared_env().await;
    let setups = ENV_SETUPS.load(Ordering::SeqCst);

    let app = TestApp::boot_plain_env::<PlainEnvApp>(env.clone(), |b| b).await;

    assert_eq!(ENV_SETUPS.load(Ordering::SeqCst), setups);
    assert_eq!(app.bean::<SharedPool>(), env);
    app.shutdown().await;
}

#[tokio::test]
async fn try_boot_with_env_surfaces_a_build_failure() {
    // The `try_` form still reports `build`-phase failures; only `setup` is
    // out of the picture.
    let err = TestApp::try_boot_with_env::<MissingConfigFileApp>((), |b| b)
        .await
        .map(|_| ())
        .expect_err("the requested config file is absent");

    assert!(
        err.to_string().contains("does-not-exist-984.yaml"),
        "unexpected error: {err}"
    );
}

// The macro knob: `#[r2e::test(app = …, env = <expr>)]` expands to
// `boot_with_env`. Both tests below share the one `OnceCell` environment, so
// the setup count stays at 1 however the runner interleaves them — that is the
// cross-test amortisation the plain `app = …` form cannot express.

#[r2e::test(app = PlainEnvApp, env = shared_env().await)]
async fn macro_env_knob_boots_on_the_shared_environment(app: TestApp) {
    assert_eq!(app.bean::<SharedPool>().generation, 1);
    assert_eq!(ENV_SETUPS.load(Ordering::SeqCst), 1);
}

#[r2e::test(app = PlainEnvApp, env = shared_env().await, with = |b| b.override_bean(Greeter { origin: "mock" }))]
async fn macro_env_knob_composes_with_the_with_hook(app: TestApp) {
    assert_eq!(app.bean::<Greeter>(), Greeter { origin: "mock" });
    assert_eq!(app.bean::<SharedPool>().generation, 1);
    assert_eq!(ENV_SETUPS.load(Ordering::SeqCst), 1);
}
