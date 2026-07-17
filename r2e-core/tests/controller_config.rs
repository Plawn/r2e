//! Controller with an `Option<T>` `#[config]` field (Tasker task #670).
//!
//! An optional config field must NOT be treated as a required key: the
//! controller registers with the key absent (the field resolves to `None`) and
//! reads `Some(v)` when the key is present. A required (non-Option) `#[config]`
//! field is covered by the bean/producer regressions in `beans.rs`.

use http_body_util::BodyExt;
use r2e_core::config::{ConfigValue, R2eConfig};
use r2e_core::http::{Body, Request, StatusCode};
use r2e_core::prelude::*;
use r2e_core::AppBuilder;
use tower::ServiceExt;

#[controller(path = "/cfg")]
pub struct OptConfigController {
    #[config("app.greeting")]
    greeting: Option<String>,
}

#[routes]
impl OptConfigController {
    #[get("/")]
    async fn greet(&self) -> String {
        self.greeting.clone().unwrap_or_else(|| "<none>".into())
    }
}

async fn body_of(config: R2eConfig) -> String {
    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .build_state()
        .await;
    let router = app.register_controller::<OptConfigController>().build();

    let resp = router
        .oneshot(Request::builder().uri("/cfg").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(body.to_vec()).unwrap()
}

#[r2e_core::test]
async fn controller_option_config_absent_registers_and_resolves_none() {
    // Empty config: registration must NOT abort on a "missing" optional key.
    let body = body_of(R2eConfig::empty()).await;
    assert_eq!(body, "<none>");
}

#[r2e_core::test]
async fn controller_option_config_present_resolves_some() {
    let mut config = R2eConfig::empty();
    config.set("app.greeting", ConfigValue::String("hi".into()));
    let body = body_of(config).await;
    assert_eq!(body, "hi");
}
