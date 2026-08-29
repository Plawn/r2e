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
