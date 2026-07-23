//! Post-construct / pre-destroy on plugin-provided beans.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use r2e_core::plugin::{PluginInstallContext, PreStatePlugin};
use r2e_core::{AppBuilder, PostConstruct, PreDestroy};

// ── Provided-bean lifecycle: plugin post-construct / pre-destroy (Phase 5) ──

type LifecycleLog = Arc<Mutex<Vec<&'static str>>>;

/// A plugin-provided bean opting into a post-construct hook.
#[derive(Clone)]
struct InitBean {
    log: LifecycleLog,
}

impl PostConstruct for InitBean {
    fn post_construct(&self) -> r2e_core::lifecycle::LifecycleFuture<'_> {
        Box::pin(async move {
            self.log.lock().unwrap().push("bean-post-construct");
            Ok(())
        })
    }
}

/// Provides `InitBean` and opts it into a post-construct hook via the install
/// context.
struct PostConstructPlugin {
    log: LifecycleLog,
}

impl PreStatePlugin for PostConstructPlugin {
    type Provided = (InitBean,);
    type Deps = ();
    type Config = ();

    fn install(&mut self, ctx: &mut PluginInstallContext<'_>) -> (InitBean,) {
        ctx.run_post_construct::<InitBean>();
        (InitBean {
            log: self.log.clone(),
        },)
    }
}

#[r2e_core::test]
async fn plugin_run_post_construct_fires_at_build_state() {
    let log: LifecycleLog = Arc::new(Mutex::new(Vec::new()));
    let _app = AppBuilder::new()
        .plugin(PostConstructPlugin { log: log.clone() })
        .build_state()
        .await;

    assert_eq!(*log.lock().unwrap(), vec!["bean-post-construct"]);
}

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

/// Provides `DisposeBean`, opts it into disposal, and also registers a plugin
/// async shutdown hook so we can observe the documented ordering.
struct DisposePlugin {
    log: LifecycleLog,
}

impl PreStatePlugin for DisposePlugin {
    type Provided = (DisposeBean,);
    type Deps = ();
    type Config = ();

    fn install(&mut self, ctx: &mut PluginInstallContext<'_>) -> (DisposeBean,) {
        let log = self.log.clone();
        ctx.on_shutdown_async(move || {
            let log = log.clone();
            async move {
                log.lock().unwrap().push("plugin-async-shutdown");
            }
        });
        ctx.run_pre_destroy::<DisposeBean>();
        (DisposeBean {
            log: self.log.clone(),
        },)
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
    let server = tokio::spawn(async move {
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
