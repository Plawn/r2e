//! The three effect stages a plugin's `build` can register into — **Graph**
//! (`add_layer`, `after_build`, `store_data`, `on_serve`), **Routes**
//! (`after_routes`) and **Finalize** (`wrap_router`) — their order relative to
//! each other, install order within a stage, and what the `enabled` gate drops.

use std::sync::{Arc, Mutex};

use r2e_core::http::StatusCode;
use r2e_core::plugin::{Plugin, PluginBuildContext, PluginBuildError};
use r2e_core::prelude::{controller, routes};
use r2e_core::{AppBuilder, RegisterController};

/// Append-only log of `String`s (the `EventLog` fixture is `&'static str`).
#[derive(Clone, Default)]
struct Trace(Arc<Mutex<Vec<String>>>);

impl Trace {
    fn push(&self, event: impl Into<String>) {
        self.0.lock().unwrap().push(event.into());
    }
    fn entries(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

/// Registers one effect in each of the three stages, tagged with its name.
///
/// A plugin **type** may only be installed once (its group bean would collide),
/// so the install-order test below needs two distinct types: the macro stamps
/// out one per name.
macro_rules! stage_plugin {
    ($Ty:ident = $name:literal) => {
        struct $Ty(Trace);

        impl Plugin for $Ty {
            type Provided = ();
            type Deps = ();
            type Config = ();
            type Controllers = ();

            async fn build(
                self,
                _deps: (),
                _config: Option<()>,
                ctx: &mut PluginBuildContext,
            ) -> Result<(), PluginBuildError> {
                let trace = self.0;

                let t = trace.clone();
                ctx.after_build(move |_| t.push(concat!("graph:", $name)));

                let t = trace.clone();
                ctx.after_routes(move |_| t.push(concat!("routes:", $name)));

                let t = trace.clone();
                ctx.wrap_router(move |router| {
                    t.push(concat!("finalize:", $name));
                    router
                });

                Ok(())
            }
        }
    };
}

stage_plugin!(StageA = "a");
stage_plugin!(StageB = "b");

#[r2e_core::test]
async fn stages_run_graph_then_routes_then_finalize() {
    let trace = Trace::default();
    let _router = AppBuilder::new()
        .plugin(StageA(trace.clone()))
        .build_state()
        .await
        .build();

    assert_eq!(
        trace.entries(),
        vec!["graph:a", "routes:a", "finalize:a"],
        "Graph drains in build_state(), Routes then Finalize in build()"
    );
}

#[r2e_core::test]
async fn within_a_stage_effects_apply_in_install_order() {
    let trace = Trace::default();
    let _router = AppBuilder::new()
        .plugin(StageA(trace.clone()))
        .plugin(StageB(trace.clone()))
        .build_state()
        .await
        .build();

    assert_eq!(
        trace.entries(),
        vec![
            // Graph stage, install order…
            "graph:a",
            "graph:b",
            // …then the whole Routes stage, install order…
            "routes:a",
            "routes:b",
            // …then the whole Finalize stage, install order.
            "finalize:a",
            "finalize:b",
        ],
        "stages are global phases; install order only orders within a stage"
    );
}

/// Mounts a route from the Routes stage and a marker header from Finalize, so
/// a test can see both from the outside.
struct RoutesAndFinalize;

impl Plugin for RoutesAndFinalize {
    type Provided = ();
    type Deps = ();
    type Config = ();
    type Controllers = ();
    const CONFIG_PREFIX: Option<&'static str> = Some("staged");

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(), PluginBuildError> {
        ctx.after_routes(|routes| {
            routes.register_routes(r2e_core::http::Router::new().route(
                "/from-routes",
                r2e_core::http::routing::get(|| async { "routes-ok" }),
            ));
        });
        ctx.wrap_router(|router| {
            router.layer(r2e_core::http::middleware::from_fn(
                |req, next: r2e_core::http::middleware::Next| async move {
                    let mut resp = next.run(req).await;
                    resp.headers_mut()
                        .insert("x-finalize", "applied".parse().unwrap());
                    resp
                },
            ))
        });
        Ok(())
    }
}

#[r2e_core::test]
async fn routes_stage_mounts_a_router_and_finalize_wraps_it() {
    let router = AppBuilder::new()
        .plugin(RoutesAndFinalize)
        .build_state()
        .await
        .build();

    let resp = crate::support::raw_get_with(router, "/from-routes", &[]).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["x-finalize"], "applied");
}

#[r2e_core::test]
async fn a_disabled_plugin_drops_routes_and_finalize_effects_too() {
    let config = r2e_core::R2eConfig::from_yaml_str("staged:\n  enabled: false\n").unwrap();
    let router = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(RoutesAndFinalize)
        .build_state()
        .await
        .build();

    let resp = crate::support::raw_get_with(router, "/from-routes", &[]).await;
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "the Routes-stage router is dropped with the rest of the effects"
    );
    assert!(
        resp.headers().get("x-finalize").is_none(),
        "the Finalize-stage wrap is dropped too"
    );
}

// ── The route registry an `after_routes` effect reads ───────────────────────

/// Captures the route registry the Routes stage sees.
struct RegistrySpy(Arc<Mutex<Vec<String>>>);

impl Plugin for RegistrySpy {
    type Provided = ();
    type Deps = ();
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        ctx: &mut PluginBuildContext,
    ) -> Result<(), PluginBuildError> {
        let seen = Arc::clone(&self.0);
        ctx.after_routes(move |routes| {
            let mut seen = seen.lock().unwrap();
            for route in routes.routes() {
                seen.push(format!("{} {}", route.method, route.path));
            }
        });
        Ok(())
    }
}

#[r2e_core::test]
async fn after_routes_sees_controllers_registered_after_the_plugin() {
    // The plugin is installed BEFORE the controller is registered — the whole
    // point of the Routes stage is that install order does not matter.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let _router = AppBuilder::new()
        .plugin(RegistrySpy(Arc::clone(&seen)))
        .build_state()
        .await
        .register_controller::<LateController>()
        .build();

    let seen = seen.lock().unwrap().clone();
    assert!(
        seen.iter().any(|r| r == "GET /late/ping"),
        "route registry should contain the later-registered controller's route, got {seen:?}"
    );
}

#[controller(path = "/late")]
struct LateController {}

#[routes]
impl LateController {
    #[get("/ping")]
    async fn ping(&self) -> &'static str {
        "pong"
    }
}
