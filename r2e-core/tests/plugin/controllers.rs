//! `Plugin::Controllers` — controllers a plugin ships, registered by the
//! framework through the same deferred-controller machinery feature modules
//! use, with the same compile-time dependency check.

use r2e_core::http::StatusCode;
use r2e_core::plugin::{Plugin, PluginBuildContext, PluginBuildError};
use r2e_core::prelude::{controller, routes, Json};
use r2e_core::AppBuilder;
use r2e_core::RegisterController;

use crate::support::send_get;

/// A bean the plugin itself provides — its controller injects it.
#[derive(Clone)]
pub struct MetricsRegistry {
    pub label: String,
}

/// An application bean (not the plugin's) — the same controller injects it too,
/// proving a plugin controller can reach anything in the final provision list.
#[derive(Clone)]
pub struct AppSalt(pub u32);

#[controller(path = "/metrics")]
pub struct MetricsController {
    #[inject]
    registry: MetricsRegistry,
    #[inject]
    salt: AppSalt,
}

#[routes]
impl MetricsController {
    #[get("/")]
    async fn dump(&self) -> Json<String> {
        Json(format!("{}:{}", self.registry.label, self.salt.0))
    }
}

/// Plugin that provides a bean AND ships a controller injecting it.
struct Metrics;

impl Plugin for Metrics {
    type Provided = (MetricsRegistry,);
    type Deps = ();
    type Config = ();
    type Controllers = (MetricsController,);

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        _ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        Ok((MetricsRegistry {
            label: "hits".to_string(),
        },))
    }
}

#[r2e_core::test]
async fn plugin_controller_is_registered_and_injects_provided_and_app_beans() {
    let router = AppBuilder::new()
        .provide(AppSalt(42))
        .plugin(Metrics)
        .build_state()
        .await
        .build();

    let (status, body) = send_get(router, "/metrics").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "\"hits:42\"");
}

// ── A second controller, and coexistence with app controllers ───────────────

#[controller(path = "/metrics-lite")]
pub struct LiteController {
    #[inject]
    registry: MetricsRegistry,
}

#[routes]
impl LiteController {
    #[get("/")]
    async fn label(&self) -> String {
        format!("lite:{}", self.registry.label)
    }
}

#[controller(path = "/app")]
pub struct AppController {
    #[inject]
    salt: AppSalt,
}

#[routes]
impl AppController {
    #[get("/")]
    async fn salt(&self) -> Json<u32> {
        Json(self.salt.0)
    }
}

struct MetricsPair;

impl Plugin for MetricsPair {
    type Provided = (MetricsRegistry,);
    type Deps = ();
    type Config = ();
    type Controllers = (MetricsController, LiteController);

    async fn build(
        self,
        _deps: (),
        _config: Option<()>,
        _ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        Ok((MetricsRegistry {
            label: "pair".to_string(),
        },))
    }
}

#[r2e_core::test]
async fn several_plugin_controllers_coexist_with_the_apps_own() {
    let router = AppBuilder::new()
        .provide(AppSalt(7))
        .plugin(MetricsPair)
        .build_state()
        .await
        .register_controller::<AppController>()
        .build();

    let (status, body) = send_get(router.clone(), "/metrics").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "\"pair:7\"");

    let (status, body) = send_get(router.clone(), "/metrics-lite").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "lite:pair");

    let (status, body) = send_get(router, "/app").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "7");
}

/// A plugin controller's routes are part of the route registry the Routes
/// stage reads, so an `after_routes` effect documents them like any other.
struct RegistrySpy(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

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
        let seen = std::sync::Arc::clone(&self.0);
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
async fn plugin_controller_routes_land_in_the_route_registry() {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let _router = AppBuilder::new()
        .provide(AppSalt(1))
        .plugin(RegistrySpy(std::sync::Arc::clone(&seen)))
        .plugin(Metrics)
        .build_state()
        .await
        .build();

    let seen = seen.lock().unwrap().clone();
    assert!(
        seen.iter().any(|r| r == "GET /metrics/"),
        "plugin controller routes should be visible to `after_routes`, got {seen:?}"
    );
}
