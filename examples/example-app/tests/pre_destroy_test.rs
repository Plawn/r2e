//! `#[pre_destroy]` disposal hooks (W5) — the `@PreDestroy` counterpart of
//! `#[post_construct]`, on `#[bean]` impls AND `#[routes]` controller impls.
//!
//! Semantics proven here:
//! - hooks run during graceful shutdown (serve + `StopHandle::stop`);
//! - an `Err` is logged and swallowed — shutdown still completes cleanly;
//! - a pinned `override_bean` SKIPS the bean's hook (undecorated pin rule);
//! - controllers dispose before the beans they inject.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use r2e::config::R2eConfig;
use r2e::prelude::*;

// ─── Shared disposal log ───

#[derive(Clone, Default)]
pub struct Log(Arc<Mutex<Vec<String>>>);

impl Log {
    fn record(&self, s: &str) {
        self.0.lock().unwrap().push(s.to_string());
    }
    fn entries(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

// ─── A bean with a `#[pre_destroy]` hook ───

#[derive(Clone)]
pub struct Closer {
    log: Log,
}

#[bean]
impl Closer {
    pub fn new(log: Log) -> Self {
        Self { log }
    }

    #[pre_destroy]
    async fn close(&self) {
        self.log.record("bean-close");
    }
}

// ─── A bean whose `#[pre_destroy]` returns Err (must not abort shutdown) ───

#[derive(Clone)]
pub struct FailCloser {
    log: Log,
}

#[bean]
impl FailCloser {
    pub fn new(log: Log) -> Self {
        Self { log }
    }

    #[pre_destroy]
    async fn close(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.log.record("fail-close-attempted");
        Err("disposal boom".into())
    }
}

// ─── A controller with a `#[pre_destroy]` hook + a route ───

#[controller(path = "/pd")]
pub struct PdController {
    #[inject]
    log: Log,
}

#[routes]
impl PdController {
    #[get("/")]
    async fn root(&self) -> Json<&'static str> {
        Json("ok")
    }

    #[pre_destroy]
    async fn on_shutdown(&self) {
        self.log.record("controller-close");
    }
}

// ─── Serve + stop helper ───

async fn serve_then_stop<T: Clone + Send + Sync + 'static>(prepared: r2e::builder::PreparedApp<T>) {
    let stop = prepared.stop_handle();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = tokio::spawn(async move {
        prepared
            .run_with_listener(listener)
            .await
            .map_err(|e| e.to_string())
    });
    // Give startup a beat, then trigger graceful shutdown.
    tokio::time::sleep(Duration::from_millis(50)).await;
    stop.stop();
    let result = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server did not stop within 5s")
        .expect("server task panicked");
    assert!(result.is_ok(), "run() returned an error: {result:?}");
}

// ─── Tests ───

#[r2e::test]
async fn bean_pre_destroy_runs_on_shutdown() {
    let log = Log::default();
    let app = AppBuilder::new()
        .override_config(R2eConfig::empty())
        .load_config::<()>()
        .provide(log.clone())
        .register::<Closer>()
        .build_state()
        .await;
    serve_then_stop(app.prepare("127.0.0.1:0")).await;

    assert_eq!(log.entries(), vec!["bean-close"]);
}

#[r2e::test]
async fn controller_pre_destroy_runs_before_bean() {
    let log = Log::default();
    let app = AppBuilder::new()
        .override_config(R2eConfig::empty())
        .load_config::<()>()
        .provide(log.clone())
        .register::<Closer>()
        .build_state()
        .await
        .register_controller::<PdController>();
    serve_then_stop(app.prepare("127.0.0.1:0")).await;

    // Controller disposes before the beans it injected.
    assert_eq!(log.entries(), vec!["controller-close", "bean-close"]);
}

#[r2e::test]
async fn pre_destroy_err_is_logged_not_aborting() {
    let log = Log::default();
    let app = AppBuilder::new()
        .override_config(R2eConfig::empty())
        .load_config::<()>()
        .provide(log.clone())
        .register::<FailCloser>()
        .register::<Closer>()
        .build_state()
        .await;
    // Shutdown must still complete cleanly despite the failing disposer.
    serve_then_stop(app.prepare("127.0.0.1:0")).await;

    let entries = log.entries();
    assert!(
        entries.contains(&"fail-close-attempted".to_string()),
        "failing disposer must have run: {entries:?}"
    );
    assert!(
        entries.contains(&"bean-close".to_string()),
        "a later disposer must still run after an Err: {entries:?}"
    );
}

#[r2e::test]
async fn override_bean_skips_pre_destroy() {
    let log = Log::default();
    let app = AppBuilder::new()
        .override_config(R2eConfig::empty())
        .load_config::<()>()
        .provide(log.clone())
        // Pin the bean: its registration hooks (including register_pre_destroy)
        // are skipped, so the disposal hook must NOT run.
        .override_bean(Closer::new(log.clone()))
        .register::<Closer>()
        .build_state()
        .await;
    serve_then_stop(app.prepare("127.0.0.1:0")).await;

    assert!(
        log.entries().is_empty(),
        "pinned override must skip #[pre_destroy]: {:?}",
        log.entries()
    );
}
