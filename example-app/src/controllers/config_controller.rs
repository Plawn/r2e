use crate::state::Services;

#[derive(quarlus_macros::Controller)]
#[controller(state = Services)]
pub struct ConfigController {
    #[config("app.name")]
    app_name: String,

    #[config("app.version")]
    app_version: String,
}

#[quarlus_macros::routes]
impl ConfigController {
    #[get("/config")]
    async fn config_info(&self) -> axum::Json<serde_json::Value> {
        axum::Json(serde_json::json!({
            "app_name": self.app_name,
            "app_version": self.app_version,
        }))
    }
}
