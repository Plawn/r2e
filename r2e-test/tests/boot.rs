//! `TestApp::boot::<A>()` / `boot_with`: booting an `App`, pinned mocks,
//! config overrides, bean access.

use r2e_core::config::{ConfigValue, R2eConfig};
use r2e_core::{App, AppBuilder, BootableApp};
use r2e_test::TestApp;

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
/// IS a startup under `TestApp::boot`), unlike `#[pre_destroy]`, which needs a
/// shutdown and therefore never fires here.
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
