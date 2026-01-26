use crate::state::Services;

quarlus_macros::controller! {
    impl ConfigController for Services {
        #[config("app.name")]
        app_name: String,

        #[config("app.version")]
        app_version: String,

        #[get("/config")]
        async fn config_info(&self) -> axum::Json<serde_json::Value> {
            axum::Json(serde_json::json!({
                "app_name": self.app_name,
                "app_version": self.app_version,
            }))
        }
    }
}
