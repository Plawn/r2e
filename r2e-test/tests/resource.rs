//! `TestResource`: started before the boot, wired into the builder, owned by
//! the `TestApp` — through `resources(...)` on the test attributes and by hand.
//!
//! The macros resolve `r2e-test` as `crate::` inside this package
//! (proc-macro-crate reports `FoundCrate::Itself`), so what the generated code
//! names is re-exported at the test-crate root.

pub use r2e_test::{ordering, suite, TestApp, TestResources};

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use r2e_core::config::{ConfigValue, R2eConfig};
use r2e_core::{App, AppBuilder, BootableApp};
use r2e_test::TestResource;

/// Reads its upstream's address from config, like an app reading
/// `datasource.url`.
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
            config.set("upstream.url", ConfigValue::String("prod".into()));
            config.set("upstream.port", ConfigValue::Integer(80));
            b.override_config(config)
                .load_config::<()>()
                .build_state()
                .await
        })
    }
}

/// A stand-in for a container: an address and a start counter.
struct Upstream {
    url: String,
}

static UPSTREAM_STARTS: AtomicUsize = AtomicUsize::new(0);

impl TestResource for Upstream {
    async fn start() -> Self {
        let n = UPSTREAM_STARTS.fetch_add(1, Ordering::SeqCst);
        Upstream {
            url: format!("test://upstream/{n}"),
        }
    }

    fn config(&self) -> Vec<(String, String)> {
        vec![
            ("upstream.url".into(), self.url.clone()),
            // A string, read back as a number — like an env var.
            ("upstream.port".into(), "5432".into()),
        ]
    }
}

/// A second resource overriding a key the first one sets.
struct Later;

impl TestResource for Later {
    async fn start() -> Self {
        Later
    }

    fn config(&self) -> Vec<(String, String)> {
        vec![("upstream.port".into(), "6543".into())]
    }
}

/// Configures nothing: `config()` has a default.
struct Silent;

impl TestResource for Silent {
    async fn start() -> Self {
        Silent
    }
}

/// Reaches the builder itself — where a resource pins a bean it built.
struct Hooked {
    url: String,
}

impl TestResource for Hooked {
    async fn start() -> Self {
        Hooked {
            url: "from-configure".into(),
        }
    }

    fn config(&self) -> Vec<(String, String)> {
        vec![("upstream.port".into(), "7000".into())]
    }

    fn configure(&self, builder: AppBuilder) -> AppBuilder {
        builder.override_config_value("upstream.url", self.url.clone())
    }
}

// `Hooked` is declared first: its `configure` still runs after `Upstream`'s
// `config`, which sets the same key.
#[r2e::test(app = DemoApp, resources(Hooked, Upstream))]
async fn a_resource_hooks_the_builder_after_every_config(app: TestApp) {
    assert_eq!(
        app.config().get::<String>("upstream.url").unwrap(),
        "from-configure"
    );
    // Config keys keep following the declaration order.
    assert_eq!(app.config().get::<u16>("upstream.port").unwrap(), 5432);
}

#[r2e::test(
    app = DemoApp,
    resources(Hooked),
    with = |b| b.override_config_value("upstream.url", "from-with"),
)]
async fn the_with_hook_wins_over_a_resource_hook(app: TestApp) {
    assert_eq!(
        app.config().get::<String>("upstream.url").unwrap(),
        "from-with"
    );
    assert_eq!(app.config().get::<u16>("upstream.port").unwrap(), 7000);
}

#[r2e::test(app = DemoApp, resources(Upstream))]
async fn a_resource_configures_the_booted_app(app: TestApp) {
    let url = app.config().get::<String>("upstream.url").unwrap();
    assert_eq!(url, app.resource::<Upstream>().url);
    assert!(url.starts_with("test://upstream/"));
    assert_eq!(app.config().get::<u16>("upstream.port").unwrap(), 5432);
}

#[r2e::test(app = DemoApp, resources(Upstream, Later, Silent))]
async fn resources_apply_in_declaration_order(app: TestApp) {
    assert_eq!(app.config().get::<u16>("upstream.port").unwrap(), 6543);
    // Every declared resource is reachable by type.
    let _: &Silent = app.resource();
    let _: &Later = app.resource();
}

#[r2e::test(
    app = DemoApp,
    resources(Upstream),
    with = |b| b.override_config_value("upstream.url", "from-with"),
)]
async fn the_with_hook_wins_over_a_resource(app: TestApp) {
    assert_eq!(
        app.config().get::<String>("upstream.url").unwrap(),
        "from-with"
    );
    // …on the key it sets only.
    assert_eq!(app.config().get::<u16>("upstream.port").unwrap(), 5432);
}

#[r2e::test(app = DemoApp, resources(Upstream), jwt = false, env = ())]
async fn resources_compose_with_the_other_boot_arguments(app: TestApp) {
    assert!(app
        .config()
        .get::<String>("upstream.url")
        .unwrap()
        .starts_with("test://upstream/"));
}

#[r2e::test(app = DemoApp, resources(Silent))]
#[should_panic(expected = "was not booted on a `resource::Upstream` resource")]
async fn an_undeclared_resource_is_a_clear_panic(app: TestApp) {
    let _ = app.resource::<Upstream>();
}

#[r2e::test]
#[should_panic(expected = "`resource::Silent` is declared twice")]
async fn a_resource_cannot_be_started_twice() {
    let _ = TestResources::new()
        .start::<Silent>()
        .await
        .start::<Silent>()
        .await;
}

// ── Lifetime ────────────────────────────────────────────────────────────

static EVENTS: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

struct Tracked;

impl TestResource for Tracked {
    async fn start() -> Self {
        EVENTS.lock().unwrap().push("resource started");
        Tracked
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        EVENTS.lock().unwrap().push("resource dropped");
    }
}

/// Logs when the app's bean graph goes away.
#[derive(Clone)]
struct AppMarker(#[allow(dead_code)] Arc<DropLog>);

struct DropLog;

impl Drop for DropLog {
    fn drop(&mut self) {
        EVENTS.lock().unwrap().push("app dropped");
    }
}

struct TrackedApp;

impl App for TrackedApp {
    type Env = ();

    async fn setup() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn build(
        b: AppBuilder,
        _env: (),
    ) -> Result<impl BootableApp, Box<dyn std::error::Error + Send + Sync>> {
        Ok({
            EVENTS.lock().unwrap().push("app built");
            b.provide(AppMarker(Arc::new(DropLog))).build_state().await
        })
    }
}

/// By hand, which is also what the macro expands to: the resource starts
/// before the build and outlives the app that talks to it.
#[r2e::test]
async fn a_resource_starts_before_the_app_and_outlives_it() {
    let resources = TestResources::new().start::<Tracked>().await;
    let app = TestApp::boot_with::<TrackedApp>(resources.configure(|b| b))
        .await
        .with_resources(resources);
    app.shutdown().await;

    assert_eq!(
        *EVENTS.lock().unwrap(),
        [
            "resource started",
            "app built",
            "app dropped",
            "resource dropped"
        ]
    );
}

// ── Suites: started once, for every case ────────────────────────────────

struct SuiteUpstream(String);

static SUITE_STARTS: AtomicUsize = AtomicUsize::new(0);

impl TestResource for SuiteUpstream {
    async fn start() -> Self {
        let n = SUITE_STARTS.fetch_add(1, Ordering::SeqCst);
        SuiteUpstream(format!("test://suite/{n}"))
    }

    fn config(&self) -> Vec<(String, String)> {
        vec![("upstream.url".into(), self.0.clone())]
    }
}

struct ResourceSuite {
    app: TestApp,
}

#[r2e::test_suite(app = DemoApp, resources(SuiteUpstream))]
impl ResourceSuite {
    #[before_all]
    async fn boot(app: TestApp) -> Self {
        Self { app }
    }

    #[case]
    async fn first_case_sees_the_resource(&mut self) {
        self.assert_wired();
    }

    #[case]
    async fn second_case_sees_the_same_resource(&mut self) {
        self.assert_wired();
    }
}

impl ResourceSuite {
    fn assert_wired(&self) {
        assert_eq!(SUITE_STARTS.load(Ordering::SeqCst), 1, "one start per suite");
        assert_eq!(
            self.app.config().get::<String>("upstream.url").unwrap(),
            self.app.resource::<SuiteUpstream>().0
        );
    }
}
