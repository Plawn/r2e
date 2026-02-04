use crate::state::Services;
use quarlus::prelude::*;

#[derive(Controller)]
#[controller(state = Services)]
pub struct ConfigController {
    #[config("app.name")]
    app_name: String,

    #[config("app.version")]
    app_version: String,
}

#[routes]
impl ConfigController {
    #[get("/config")]
    async fn config_info(&self) -> Json<serde_json::Value> {
        Json(serde_json::json!({
            "app_name": self.app_name,
            "app_version": self.app_version,
        }))
    }
}
