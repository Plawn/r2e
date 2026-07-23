//! The [`App`] trait and the assembly logic behind [`launch`].
//!
//! `launch` itself binds a port (via `serve_auto`), so it is not exercised
//! here; instead we drive the same seam it uses on its normal path —
//! `App::setup` → `App::build` → capture `r2e_config` — without serving.

use r2e_core::config::{ConfigValue, R2eConfig};
use r2e_core::{App, AppBuilder, BootableApp};

struct DemoApp;

impl App for DemoApp {
    type Env = i64;

    async fn setup() -> i64 {
        42
    }

    async fn build(b: AppBuilder, env: i64) -> impl BootableApp {
        let mut config = R2eConfig::empty();
        config.set("app.answer", ConfigValue::Integer(env));
        b.override_config(config)
            .load_config::<()>()
            .provide(env)
            .build_state()
            .await
    }
}

#[tokio::test]
async fn launch_normal_path_assembly() {
    // Mirror launch's non-dev path up to (but not including) serve_auto.
    let env = DemoApp::setup().await;
    assert_eq!(env, 42);

    let app = DemoApp::build(AppBuilder::new(), env).await;

    // The config capture seam the dev-reload loop feeds back between patches.
    let config = app.r2e_config().expect("config present after build");
    assert_eq!(config.get::<i64>("app.answer").unwrap(), 42);

    // The environment made it into the resolved bean graph.
    assert_eq!(app.bean_context().get::<i64>(), 42);
}
