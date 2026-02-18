use std::sync::Arc;

use r2e::config::{ConfigValue, R2eConfig};
use r2e::prelude::*;
use r2e::r2e_security::JwtClaimsValidator;
use r2e_test::{TestApp, TestJwt};

// ─── State ───

#[derive(Clone)]
struct ConfigTestState {
    jwt_validator: Arc<JwtClaimsValidator>,
    config: R2eConfig,
}

impl r2e::http::extract::FromRef<ConfigTestState> for Arc<JwtClaimsValidator> {
    fn from_ref(state: &ConfigTestState) -> Self {
        state.jwt_validator.clone()
    }
}

impl r2e::http::extract::FromRef<ConfigTestState> for R2eConfig {
    fn from_ref(state: &ConfigTestState) -> Self {
        state.config.clone()
    }
}

// ─── Controller testing various config types ───

#[derive(Controller)]
#[controller(path = "/config", state = ConfigTestState)]
pub struct ConfigTestController {
    #[config("app.name")]
    name: String,

    #[config("app.port")]
    port: i64,

    #[config("app.rate")]
    rate: f64,

    #[config("app.debug")]
    debug: bool,

    #[config("app.optional_value")]
    optional_value: Option<String>,
}

#[routes]
impl ConfigTestController {
    #[get("/string")]
    async fn get_string(
        &self,
        #[inject(identity)] _user: r2e::r2e_security::AuthenticatedUser,
    ) -> Json<String> {
        Json(self.name.clone())
    }

    #[get("/i64")]
    async fn get_i64(
        &self,
        #[inject(identity)] _user: r2e::r2e_security::AuthenticatedUser,
    ) -> Json<i64> {
        Json(self.port)
    }

    #[get("/f64")]
    async fn get_f64(
        &self,
        #[inject(identity)] _user: r2e::r2e_security::AuthenticatedUser,
    ) -> Json<f64> {
        Json(self.rate)
    }

    #[get("/bool")]
    async fn get_bool(
        &self,
        #[inject(identity)] _user: r2e::r2e_security::AuthenticatedUser,
    ) -> Json<bool> {
        Json(self.debug)
    }

    #[get("/option")]
    async fn get_option(
        &self,
        #[inject(identity)] _user: r2e::r2e_security::AuthenticatedUser,
    ) -> Json<serde_json::Value> {
        Json(serde_json::json!({ "value": self.optional_value }))
    }
}

fn make_config_with_option(option_value: Option<&str>) -> R2eConfig {
    let mut config = R2eConfig::empty();
    config.set("app.name", ConfigValue::String("MyApp".into()));
    config.set("app.port", ConfigValue::Integer(8080));
    config.set("app.rate", ConfigValue::Float(3.14));
    config.set("app.debug", ConfigValue::Bool(true));
    // Option<String> field: set it if provided, use Null otherwise.
    // The validation still requires the key to exist, but Null → None at runtime.
    match option_value {
        Some(v) => config.set("app.optional_value", ConfigValue::String(v.into())),
        None => config.set("app.optional_value", ConfigValue::Null),
    }
    config
}

async fn setup_with_option(option_value: Option<&str>) -> (TestApp, TestJwt) {
    let jwt = TestJwt::new();
    let config = make_config_with_option(option_value);

    let state = ConfigTestState {
        jwt_validator: Arc::new(jwt.claims_validator()),
        config: config.clone(),
    };

    let app = TestApp::from_builder(
        AppBuilder::new()
            .with_state(state)
            .with_config(config)
            .with(ErrorHandling)
            .register_controller::<ConfigTestController>(),
    );

    (app, jwt)
}

async fn setup() -> (TestApp, TestJwt) {
    setup_with_option(Some("I exist")).await
}

// ─── Tests ───

#[tokio::test]
async fn test_config_string_injection() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    let resp = app
        .get("/config/string")
        .bearer(&token)
        .send()
        .await
        .assert_ok();
    let name: String = resp.json();
    assert_eq!(name, "MyApp");
}

#[tokio::test]
async fn test_config_i64_injection() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    let resp = app
        .get("/config/i64")
        .bearer(&token)
        .send()
        .await
        .assert_ok();
    let port: i64 = resp.json();
    assert_eq!(port, 8080);
}

#[tokio::test]
async fn test_config_f64_injection() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    let resp = app
        .get("/config/f64")
        .bearer(&token)
        .send()
        .await
        .assert_ok();
    let rate: f64 = resp.json();
    assert!((rate - 3.14).abs() < f64::EPSILON);
}

#[tokio::test]
async fn test_config_bool_injection() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    let resp = app
        .get("/config/bool")
        .bearer(&token)
        .send()
        .await
        .assert_ok();
    let debug: bool = resp.json();
    assert!(debug);
}

#[tokio::test]
async fn test_config_option_some() {
    let (app, jwt) = setup_with_option(Some("I exist")).await;
    let token = jwt.token("user-1", &["user"]);
    let resp = app
        .get("/config/option")
        .bearer(&token)
        .send()
        .await
        .assert_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["value"], "I exist");
}

#[tokio::test]
async fn test_config_option_none() {
    let (app, jwt) = setup_with_option(None).await;
    let token = jwt.token("user-1", &["user"]);
    let resp = app
        .get("/config/option")
        .bearer(&token)
        .send()
        .await
        .assert_ok();
    let body: serde_json::Value = resp.json();
    assert!(body["value"].is_null());
}

// ─── Missing required config key panics ───

#[derive(Controller)]
#[controller(path = "/bad-config", state = ConfigTestState)]
pub struct MissingConfigController {
    #[config("nonexistent.key")]
    required_value: String,
}

#[routes]
impl MissingConfigController {
    #[get("/")]
    async fn get_value(
        &self,
        #[inject(identity)] _user: r2e::r2e_security::AuthenticatedUser,
    ) -> Json<String> {
        Json(self.required_value.clone())
    }
}

#[tokio::test]
#[should_panic(expected = "CONFIGURATION ERRORS")]
async fn test_config_missing_required_panics() {
    let jwt = TestJwt::new();
    let config = R2eConfig::empty(); // no keys set

    let state = ConfigTestState {
        jwt_validator: Arc::new(jwt.claims_validator()),
        config: config.clone(),
    };

    // This should panic during register_controller because the config key is missing
    let _app = TestApp::from_builder(
        AppBuilder::new()
            .with_state(state)
            .with_config(config)
            .with(ErrorHandling)
            .register_controller::<MissingConfigController>(),
    );
}
