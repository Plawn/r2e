use crate::state::Services;
use r2e::prelude::*;

/// Nested config section for app-specific settings.
#[derive(ConfigProperties, Clone, Debug)]
pub struct AppConfig {
    /// Application name
    pub name: String,

    /// Welcome greeting
    #[config(default = "Hello!")]
    pub greeting: String,

    /// Application version
    pub version: Option<String>,
}

/// Root-level config loaded via `load_config::<RootConfig>()`.
///
/// `AppConfig` is auto-registered as a bean thanks to `#[config(section)]`
/// and `register_children`, so controllers can `#[inject]` it directly.
#[derive(ConfigProperties, Clone, Debug)]
pub struct RootConfig {
    #[config(section)]
    pub app: AppConfig,
}

#[derive(Controller)]
#[controller(state = Services)]
pub struct ConfigController {
    #[inject]
    root_config: RootConfig,
}

#[routes]
impl ConfigController {
    #[get("/config")]
    async fn config_info(&self) -> Json<serde_json::Value> {
        let app = &self.root_config.app;
        Json(serde_json::json!({
            "app_name": app.name,
            "app_version": app.version,
            "greeting": app.greeting,
        }))
    }
}
