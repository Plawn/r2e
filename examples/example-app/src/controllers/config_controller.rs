use crate::state::Services;
use r2e::prelude::*;

#[derive(ConfigProperties, Clone, Debug)]
#[config(prefix = "app")]
pub struct AppConfig {
    /// Application name
    pub name: String,

    /// Welcome greeting
    #[config(default = "Hello!")]
    pub greeting: String,

    /// Application version
    pub version: Option<String>,
}

#[derive(Controller)]
#[controller(state = Services)]
pub struct ConfigController {
    #[config_section]
    app_config: AppConfig,
}

#[routes]
impl ConfigController {
    #[get("/config")]
    async fn config_info(&self) -> Json<serde_json::Value> {
        Json(serde_json::json!({
            "app_name": self.app_config.name,
            "app_version": self.app_config.version,
            "greeting": self.app_config.greeting,
        }))
    }
}
