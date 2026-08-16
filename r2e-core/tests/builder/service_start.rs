use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use r2e_core::config::{ConfigValue, R2eConfig};
use r2e_core::{AppBuilder, BeanContext, ServiceComponent, SpawnService};
use tokio_util::sync::CancellationToken;

static STARTED: AtomicUsize = AtomicUsize::new(0);
static STOPPED: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone)]
struct ProbeService;

impl ServiceComponent for ProbeService {
    type Deps = r2e_core::type_list::TCons<ProbeService, r2e_core::type_list::TNil>;

    fn from_context(ctx: &BeanContext) -> Self {
        ctx.get::<ProbeService>()
    }

    async fn start(self, shutdown: CancellationToken) {
        STARTED.fetch_add(1, Ordering::SeqCst);
        shutdown.cancelled().await;
        STOPPED.fetch_add(1, Ordering::SeqCst);
    }
}

#[r2e_macros::producer(start)]
fn make_probe_service() -> ProbeService {
    ProbeService
}

#[tokio::test]
async fn producer_start_runs_output_as_tracked_service() {
    STARTED.store(0, Ordering::SeqCst);
    STOPPED.store(0, Ordering::SeqCst);

    let app = AppBuilder::new()
        .register::<MakeProbeService>()
        .build_state()
        .await;
    let prepared = app.prepare("127.0.0.1:0");
    let stop = prepared.stop_handle();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = r2e_core::rt::spawn(async move {
        prepared
            .run_with_listener(listener)
            .await
            .map_err(|e| e.to_string())
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(STARTED.load(Ordering::SeqCst), 1);

    stop.stop();
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(STOPPED.load(Ordering::SeqCst), 1);
}

// ── Config keys declared by #[derive(BackgroundService)] ───────────────────
//
// A background service is not a `Bean`, so its `#[config]` / `#[live_config]`
// fields used to escape aggregated startup validation entirely and blow up
// late inside `from_context`. The derive now emits
// `ServiceComponent::config_keys()`, and the registration path that owns the
// service validates them: `spawn_service` for the explicit form,
// `BeanRegistry::resolve_reusing` for `#[producer(start)]` outputs.

#[derive(r2e_macros::BackgroundService)]
struct ConfiguredService {
    #[config("svc.interval")]
    #[allow(dead_code)]
    interval: u64,
    #[config("svc.optional")]
    #[allow(dead_code)]
    optional: Option<u64>,
}

impl ConfiguredService {
    async fn run(&self, shutdown: CancellationToken) {
        shutdown.cancelled().await;
    }
}

/// Only `Required` keys are presence-validated. `Optional` is declared for
/// completeness (the kind is what a host would fingerprint — a background
/// service has no fingerprint of its own) but is never reported as missing.
#[test]
fn background_service_declares_its_config_keys() {
    let keys = <ConfiguredService as ServiceComponent>::config_keys();
    let mut named: Vec<(&str, bool)> = keys
        .iter()
        .map(|(key, _, kind)| (*key, kind.is_required()))
        .collect();
    named.sort();
    assert_eq!(named, vec![("svc.interval", true), ("svc.optional", false)]);
}

#[tokio::test]
async fn missing_service_config_key_fails_spawn_service() {
    let app = AppBuilder::new()
        .override_config(R2eConfig::empty())
        .load_config::<()>()
        .build_state()
        .await;

    let err = app
        .try_spawn_service::<ConfiguredService>()
        .err()
        .expect("a background service's missing #[config] key must fail registration");
    let msg = err.to_string();

    assert!(
        msg.contains("svc.interval"),
        "the missing key must be named: {msg}"
    );
    assert!(
        msg.contains("ConfiguredService"),
        "the report must name the service that requires it: {msg}"
    );
    assert!(!msg.contains("svc.optional"), "{msg}");
}

#[tokio::test]
async fn present_service_config_key_passes_spawn_service() {
    let mut config = R2eConfig::empty();
    config.set("svc.interval", ConfigValue::Integer(30));

    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .build_state()
        .await;

    let app = app
        .try_spawn_service::<ConfiguredService>()
        .expect("all required keys present");
    let _ = app.build();
}

// ── The same, through the `#[producer(start)]` path ────────────────────────

#[derive(Clone, r2e_macros::BackgroundService)]
struct ProducedService {
    #[config("produced.interval")]
    #[allow(dead_code)]
    interval: u64,
}

impl ProducedService {
    async fn run(&self, shutdown: CancellationToken) {
        shutdown.cancelled().await;
    }
}

#[r2e_macros::producer(start)]
fn make_produced_service() -> ProducedService {
    ProducedService { interval: 0 }
}

/// `#[producer(start)]` hands the output to the service runner without ever
/// passing through `spawn_service`, so the declaration is validated where the
/// source is registered: during graph resolution, as an aggregated
/// `BeanError::MissingConfigKeys`.
#[tokio::test]
async fn missing_config_key_of_producer_start_service_fails_build_state() {
    let err = AppBuilder::new()
        .override_config(R2eConfig::empty())
        .load_config::<()>()
        .register::<MakeProducedService>()
        .try_build_state()
        .await
        .err()
        .expect("a #[producer(start)] service's missing #[config] key must fail build_state");
    let msg = err.to_string();

    assert!(msg.contains("produced.interval"), "{msg}");
    assert!(msg.contains("ProducedService"), "{msg}");
}

// ── One report for the whole graph ─────────────────────────────────────────
//
// Service declarations used to be validated with an early `?` of their own,
// several statements before the bean keys were checked: an app missing both
// only ever saw the service half, fixed it, rebooted, and then saw the bean
// half. `BeanRegistry::validate_all_config` merges both into one
// `BeanError::MissingConfigKeys`.

#[derive(Clone, r2e_core::prelude::Bean)]
struct ConfiguredBean {
    #[config("bean.title")]
    #[allow(dead_code)]
    title: String,
}

#[tokio::test]
async fn missing_bean_and_service_config_keys_are_reported_together() {
    let err = AppBuilder::new()
        .override_config(R2eConfig::empty())
        .load_config::<()>()
        .register::<ConfiguredBean>()
        .register::<MakeProducedService>()
        .try_build_state()
        .await
        .err()
        .expect("both missing keys must fail build_state");
    let msg = err.to_string();

    assert!(
        msg.contains("bean.title"),
        "the bean's missing key must be in the same report: {msg}"
    );
    assert!(
        msg.contains("produced.interval"),
        "the service's missing key must be in the same report: {msg}"
    );
}

// ── `#[config_section]` on a background service ────────────────────────────
//
// A section declares only its prefix in `config_keys()` (kind `Section`, never
// presence-validated), so a service reading a section used to boot and blow up
// inside `from_context`. The derive now also emits
// `ServiceComponent::config_sections()` — a `SectionValidator` per field —
// which both registration paths run through `validate_declared_sections`.

#[derive(Clone, r2e_core::prelude::ConfigProperties)]
struct SvcSection {
    #[allow(dead_code)]
    window: u64,
}

#[derive(Clone, r2e_macros::BackgroundService)]
struct SectionService {
    #[config_section(prefix = "svc.quota")]
    #[allow(dead_code)]
    quota: SvcSection,
}

impl SectionService {
    async fn run(&self, shutdown: CancellationToken) {
        shutdown.cancelled().await;
    }
}

#[r2e_macros::producer(start)]
fn make_section_service() -> SectionService {
    SectionService {
        quota: SvcSection { window: 0 },
    }
}

#[test]
fn background_service_declares_its_config_sections() {
    let sections = <SectionService as ServiceComponent>::config_sections();
    let prefixes: Vec<&str> = sections.iter().map(|s| s.prefix()).collect();
    assert_eq!(prefixes, vec!["svc.quota"]);

    // The prefix is also declared as a `Section` key — that entry is what a
    // host would fingerprint; it is never presence-validated.
    let keys = <SectionService as ServiceComponent>::config_keys();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].0, "svc.quota");
    assert!(!keys[0].2.is_required());
}

#[tokio::test]
async fn missing_service_config_section_key_fails_spawn_service() {
    let app = AppBuilder::new()
        .override_config(R2eConfig::empty())
        .load_config::<()>()
        .build_state()
        .await;

    let err = app
        .try_spawn_service::<SectionService>()
        .err()
        .expect("a background service's missing #[config_section] key must fail registration");
    let msg = err.to_string();

    assert!(
        msg.contains("svc.quota.window"),
        "the section must be walked, not just its prefix: {msg}"
    );
}

#[tokio::test]
async fn missing_config_section_key_of_producer_start_service_fails_build_state() {
    let err = AppBuilder::new()
        .override_config(R2eConfig::empty())
        .load_config::<()>()
        .register::<MakeSectionService>()
        .try_build_state()
        .await
        .err()
        .expect("a #[producer(start)] service's missing section key must fail build_state");
    let msg = err.to_string();

    assert!(msg.contains("svc.quota.window"), "{msg}");
}

#[tokio::test]
async fn present_service_config_section_passes_spawn_service() {
    let mut config = R2eConfig::empty();
    config.set("svc.quota.window", ConfigValue::Integer(60));

    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .build_state()
        .await;

    let app = app
        .try_spawn_service::<SectionService>()
        .expect("the section is complete");
    let _ = app.build();
}

// ── A dropped `run()` future still stops the service ────────────────────────
//
// `spawn_service` tasks wait on the token they were handed, and the plugin sync
// shutdown hook that cancels it runs only on the exits `run_inner` controls. A
// dropped `run()` future — exactly what an `r2e dev` hot patch does to the
// previous cycle — runs no hook at all, so a service that only ever saw a
// private token would keep running (and, since round 4, keep the bean graph
// alive) with nothing left able to stop it. The token is therefore a CHILD of
// the app shutdown root, which the run future's drop guard cancels.

static DROP_STARTED: AtomicUsize = AtomicUsize::new(0);
static DROP_STOPPED: AtomicUsize = AtomicUsize::new(0);

struct DropProbeService;

impl ServiceComponent for DropProbeService {
    type Deps = r2e_core::type_list::TNil;

    fn from_context(_ctx: &BeanContext) -> Self {
        DropProbeService
    }

    async fn start(self, shutdown: CancellationToken) {
        DROP_STARTED.fetch_add(1, Ordering::SeqCst);
        shutdown.cancelled().await;
        DROP_STOPPED.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn dropping_the_run_future_stops_a_spawn_service_task() {
    DROP_STARTED.store(0, Ordering::SeqCst);
    DROP_STOPPED.store(0, Ordering::SeqCst);

    let app = AppBuilder::new()
        .build_state()
        .await
        .spawn_service::<DropProbeService>();
    let prepared = app.prepare("127.0.0.1:0");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();

    // Own the future here rather than spawning it: dropping it below is the
    // hot-patch shape, deterministic and with no abort() in between.
    let mut server = Box::pin(prepared.run_with_listener(listener));
    tokio::select! {
        r = &mut server => panic!("the server returned before the service started: {r:?}"),
        _ = tokio::time::sleep(Duration::from_millis(100)) => {}
    }
    assert_eq!(
        DROP_STARTED.load(Ordering::SeqCst),
        1,
        "the service must be running before the future is dropped"
    );

    drop(server);

    tokio::time::timeout(Duration::from_secs(3), async {
        while DROP_STOPPED.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect(
        "dropping the run() future must cancel the service token: no shutdown \
         hook runs on that path, so the token has to be a child of the app root",
    );
}
