//! Plugin build failure (`Err` aborts boot) and pre-destroy on
//! plugin-provided beans.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use r2e_core::plugin::{PluginBuildContext, PluginBuildError, PluginSetupContext, PreStatePlugin};
use r2e_core::{AppBuilder, BeanError, PreDestroy};

use crate::fixtures::Alpha;

// ── Fallible build ───────────────────────────────────────────────────────────

/// A plugin whose `build` fails — e.g. a backend it must connect to is down.
struct FailingPlugin;

impl PreStatePlugin for FailingPlugin {
    type Provided = (Alpha,);
    type Deps = ();
    type Config = ();

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        _ctx: &mut PluginBuildContext,
    ) -> Result<(Alpha,), PluginBuildError> {
        Err("backend unreachable".into())
    }
}

#[r2e_core::test]
async fn build_err_aborts_boot_with_plugin_build_error() {
    let result = AppBuilder::new().plugin(FailingPlugin).try_build_state().await;

    let err = result.err().expect("boot must fail");
    match &err {
        BeanError::PluginBuild { plugin, source } => {
            assert_eq!(*plugin, "FailingPlugin");
            assert_eq!(source.to_string(), "backend unreachable");
        }
        other => panic!("expected PluginBuild, got: {other:?}"),
    }
    // The Display form names the plugin and carries the cause.
    let msg = err.to_string();
    assert!(msg.contains("FailingPlugin"), "names the plugin: {msg}");
    assert!(msg.contains("backend unreachable"), "carries the cause: {msg}");
}

#[r2e_core::test]
#[should_panic(expected = "FailingPlugin")]
async fn build_err_panics_through_build_state() {
    // The panicking variant surfaces the same error.
    let _app = AppBuilder::new().plugin(FailingPlugin).build_state().await;
}

// ── Pre-destroy on plugin-provided beans ────────────────────────────────────

type LifecycleLog = Arc<Mutex<Vec<&'static str>>>;

/// A plugin-provided bean with a disposal hook.
#[derive(Clone)]
struct DisposeBean {
    log: LifecycleLog,
}

impl PreDestroy for DisposeBean {
    fn pre_destroy(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            self.log.lock().unwrap().push("bean-dispose");
        })
    }
}

/// Provides `DisposeBean`, opts it into disposal in `setup` (the lifecycle
/// registrar is the setup context's remaining job), and registers a plugin
/// async shutdown hook from `build` so we can observe the documented ordering.
struct DisposePlugin {
    log: LifecycleLog,
}

impl PreStatePlugin for DisposePlugin {
    type Provided = (DisposeBean,);
    type Deps = ();
    type Config = ();

    fn setup(&mut self, ctx: &mut PluginSetupContext) {
        ctx.run_pre_destroy::<DisposeBean>();
    }

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(DisposeBean,), PluginBuildError> {
        let log = self.log.clone();
        ctx.on_shutdown_async(move || {
            let log = log.clone();
            async move {
                log.lock().unwrap().push("plugin-async-shutdown");
            }
        });
        Ok((DisposeBean {
            log: self.log.clone(),
        },))
    }
}

#[r2e_core::test]
async fn plugin_pre_destroy_runs_on_shutdown_after_plugin_hooks() {
    let log: LifecycleLog = Arc::new(Mutex::new(Vec::new()));
    let app = AppBuilder::new()
        .plugin(DisposePlugin { log: log.clone() })
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

    stop.stop();
    let result = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server did not stop within 5s")
        .expect("server task panicked");
    assert!(result.is_ok(), "run() returned an error: {result:?}");

    // Bean disposers run within the async shutdown phase, after the plugin's
    // own async shutdown hooks.
    assert_eq!(
        *log.lock().unwrap(),
        vec!["plugin-async-shutdown", "bean-dispose"]
    );
}

// ── One panicking sync shutdown hook must not silence the others ────────────
//
// Plugin sync shutdown hooks are pure signals: each one typically cancels the
// token some background task is parked on. Running them as one batch that a
// panic can abandon means every hook after the bad one never fires — its task
// is stranded for the life of the process. The hooks are therefore taken from
// the shared cell ONE AT A TIME and each runs inside `catch_unwind`.

struct PanickingShutdownHookPlugin {
    first: Arc<std::sync::atomic::AtomicBool>,
    second: Arc<std::sync::atomic::AtomicBool>,
}

impl PreStatePlugin for PanickingShutdownHookPlugin {
    type Provided = (Alpha,);
    type Deps = ();
    type Config = ();

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(Alpha,), PluginBuildError> {
        let first = self.first;
        ctx.on_shutdown(move || {
            first.store(true, std::sync::atomic::Ordering::SeqCst);
            panic!("plugin shutdown hook exploded");
        });
        let second = self.second;
        ctx.on_shutdown(move || {
            second.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        Ok((Alpha(1),))
    }
}

#[tokio::test]
async fn a_panicking_shutdown_hook_does_not_swallow_the_next_one() {
    let first = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let second = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let app = AppBuilder::new()
        .plugin(PanickingShutdownHookPlugin {
            first: first.clone(),
            second: second.clone(),
        })
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
    stop.stop();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("shutdown must complete despite the panicking hook")
        .expect("the server task must not be killed by a hook panic")
        .expect("run() must return Ok");

    assert!(
        first.load(std::sync::atomic::Ordering::SeqCst),
        "the first hook ran"
    );
    assert!(
        second.load(std::sync::atomic::Ordering::SeqCst),
        "the hook registered AFTER the panicking one must still be signalled"
    );
}
