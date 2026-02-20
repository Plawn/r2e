use std::sync::Arc;

use r2e::config::{ConfigValue, R2eConfig};
use r2e::prelude::*;
use r2e::r2e_rate_limit::RateLimit;
use r2e::r2e_security::{AuthenticatedUser, JwtClaimsValidator};
use r2e_test::{TestApp, TestJwt};
use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;

// Re-use the example app types inline since we can't import from a binary crate.

mod common {
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use r2e::r2e_events::EventBus;

    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
    pub struct User {
        pub id: u64,
        pub name: String,
        pub email: String,
    }

    #[derive(serde::Deserialize, serde::Serialize, garde::Validate, schemars::JsonSchema)]
    pub struct CreateUserRequest {
        #[garde(length(min = 1, max = 100))]
        pub name: String,
        #[garde(email)]
        pub email: String,
    }

    #[derive(Debug, Clone)]
    pub struct UserCreatedEvent {
        pub user_id: u64,
        pub name: String,
        pub email: String,
    }

    #[derive(Clone)]
    pub struct UserService {
        users: Arc<RwLock<Vec<User>>>,
        event_bus: EventBus,
    }

    impl UserService {
        pub fn new(event_bus: EventBus) -> Self {
            let users = vec![
                User { id: 1, name: "Alice".into(), email: "alice@example.com".into() },
                User { id: 2, name: "Bob".into(), email: "bob@example.com".into() },
            ];
            Self {
                users: Arc::new(RwLock::new(users)),
                event_bus,
            }
        }

        pub async fn list(&self) -> Vec<User> {
            self.users.read().await.clone()
        }

        pub async fn get_by_id(&self, id: u64) -> Option<User> {
            self.users.read().await.iter().find(|u| u.id == id).cloned()
        }

        pub async fn create(&self, name: String, email: String) -> User {
            let user = {
                let mut users = self.users.write().await;
                let id = users.len() as u64 + 1;
                let user = User { id, name, email };
                users.push(user.clone());
                user
            };
            self.event_bus
                .emit(UserCreatedEvent {
                    user_id: user.id,
                    name: user.name.clone(),
                    email: user.email.clone(),
                })
                .await;
            user
        }

        pub async fn count(&self) -> usize {
            self.users.read().await.len()
        }
    }
}

use common::*;
use r2e::r2e_events::EventBus;

#[derive(Clone)]
struct TestServices {
    user_service: UserService,
    jwt_validator: Arc<JwtClaimsValidator>,
    pool: sqlx::SqlitePool,
    event_bus: EventBus,
    config: R2eConfig,
    #[allow(dead_code)]
    cancel: CancellationToken,
    rate_limiter: r2e::r2e_rate_limit::RateLimitRegistry,
}

impl r2e::http::extract::FromRef<TestServices> for Arc<JwtClaimsValidator> {
    fn from_ref(state: &TestServices) -> Self {
        state.jwt_validator.clone()
    }
}

impl r2e::http::extract::FromRef<TestServices> for sqlx::SqlitePool {
    fn from_ref(state: &TestServices) -> Self {
        state.pool.clone()
    }
}

impl r2e::http::extract::FromRef<TestServices> for R2eConfig {
    fn from_ref(state: &TestServices) -> Self {
        state.config.clone()
    }
}

impl r2e::http::extract::FromRef<TestServices> for EventBus {
    fn from_ref(state: &TestServices) -> Self {
        state.event_bus.clone()
    }
}

impl r2e::http::extract::FromRef<TestServices> for r2e::r2e_rate_limit::RateLimitRegistry {
    fn from_ref(state: &TestServices) -> Self {
        state.rate_limiter.clone()
    }
}

#[derive(Controller)]
#[controller(state = TestServices)]
pub struct TestUserController {
    #[inject]
    user_service: UserService,

    #[inject]
    pool: sqlx::SqlitePool,

    #[identity]
    user: AuthenticatedUser,

    #[config("app.greeting")]
    greeting: String,
}

#[routes]
impl TestUserController {
    #[get("/users")]
    #[intercept(Logged::info())]
    #[intercept(Timed::info())]
    async fn list(&self) -> Json<Vec<User>> {
        let users = self.user_service.list().await;
        Json(users)
    }

    #[get("/users/{id}")]
    async fn get_by_id(
        &self,
        Path(id): Path<u64>,
    ) -> Result<Json<User>, AppError> {
        match self.user_service.get_by_id(id).await {
            Some(user) => Ok(Json(user)),
            None => Err(AppError::NotFound("User not found".into())),
        }
    }

    #[post("/users")]
    async fn create(
        &self,
        Json(body): Json<CreateUserRequest>,
    ) -> Json<User> {
        let user = self.user_service.create(body.name, body.email).await;
        Json(user)
    }

    #[get("/greeting")]
    async fn greeting(&self) -> Json<serde_json::Value> {
        Json(serde_json::json!({ "greeting": self.greeting }))
    }

    #[get("/error/custom")]
    async fn custom_error(&self) -> Result<Json<()>, AppError> {
        Err(AppError::Custom {
            status: StatusCode::from_u16(418).unwrap(),
            body: serde_json::json!({ "error": "I'm a teapot", "code": 418 }),
        })
    }

    #[get("/users/cached")]
    #[intercept(Cache::ttl(30))]
    #[intercept(Timed::info())]
    async fn cached_list(&self) -> Json<serde_json::Value> {
        let users = self.user_service.list().await;
        Json(serde_json::to_value(users).unwrap())
    }

    #[post("/users/rate-limited")]
    #[pre_guard(RateLimit::global(3, 60))]
    async fn create_rate_limited(
        &self,
        Json(body): Json<CreateUserRequest>,
    ) -> Result<Json<User>, AppError> {
        let user = self.user_service.create(body.name, body.email).await;
        Ok(Json(user))
    }

    #[get("/me")]
    async fn me(&self) -> Json<AuthenticatedUser> {
        Json(self.user.clone())
    }

    #[get("/admin/users")]
    #[roles("admin")]
    async fn admin_list(&self) -> Json<Vec<User>> {
        let users = self.user_service.list().await;
        Json(users)
    }
}

async fn setup() -> (TestApp, TestJwt) {
    let jwt = TestJwt::new();
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    let event_bus = EventBus::new();

    let mut config = R2eConfig::empty();
    config.set(
        "app.name",
        ConfigValue::String("Test App".into()),
    );
    config.set(
        "app.greeting",
        ConfigValue::String("Hello from tests!".into()),
    );
    config.set(
        "app.version",
        ConfigValue::String("0.0.1-test".into()),
    );

    let services = TestServices {
        user_service: UserService::new(event_bus.clone()),
        jwt_validator: Arc::new(jwt.claims_validator()),
        pool,
        event_bus,
        config: config.clone(),
        cancel: CancellationToken::new(),
        rate_limiter: r2e::r2e_rate_limit::RateLimitRegistry::default(),
    };

    let openapi_config =
        r2e::r2e_openapi::OpenApiConfig::new("Test API", "0.1.0").with_docs_ui(true);

    let app = TestApp::from_builder(
        AppBuilder::new()
            .with_state(services)
            .with_config(config)
            .with(Health)
            .with(ErrorHandling)
            .with(NormalizePath)
            .with(DevReload)
            .with(r2e::r2e_openapi::OpenApiPlugin::new(openapi_config))
            .register_controller::<TestUserController>(),
    );

    (app, jwt)
}

// ─── Existing tests ───

#[tokio::test]
async fn test_health_endpoint() {
    let (app, _jwt) = setup().await;
    let resp = app.get("/health").send().await.assert_ok();
    assert_eq!(resp.text(), "OK");
}

#[tokio::test]
async fn test_list_users_authenticated() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    let resp = app.get("/users").bearer(&token).send().await.assert_ok();
    let users: Vec<User> = resp.json();
    assert_eq!(users.len(), 2);
    assert_eq!(users[0].name, "Alice");
}

#[tokio::test]
async fn test_list_users_unauthenticated() {
    let (app, _jwt) = setup().await;
    app.get("/users").send().await.assert_unauthorized();
}

#[tokio::test]
async fn test_get_user_by_id() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    let resp = app.get("/users/1").bearer(&token).send().await.assert_ok();
    let user: User = resp.json();
    assert_eq!(user.name, "Alice");
}

#[tokio::test]
async fn test_get_user_not_found() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    app.get("/users/999").bearer(&token).send().await.assert_not_found();
}

#[tokio::test]
async fn test_me_endpoint() {
    let (app, jwt) = setup().await;
    let token = jwt.token_with_claims("user-42", &["user"], Some("test@example.com"));
    let resp = app.get("/me").bearer(&token).send().await.assert_ok();
    let user: AuthenticatedUser = resp.json();
    assert_eq!(user.sub, "user-42");
}

#[tokio::test]
async fn test_admin_endpoint_with_admin_role() {
    let (app, jwt) = setup().await;
    let token = jwt.token("admin-1", &["admin"]);
    let resp = app
        .get("/admin/users")
        .bearer(&token)
        .send()
        .await
        .assert_ok();
    let users: Vec<User> = resp.json();
    assert_eq!(users.len(), 2);
}

#[tokio::test]
async fn test_admin_endpoint_without_admin_role() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    app.get("/admin/users")
        .bearer(&token)
        .send()
        .await
        .assert_forbidden();
}

// ─── New tests: Configuration (#1) ───

#[tokio::test]
async fn test_config_greeting_endpoint() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    let resp = app
        .get("/greeting")
        .bearer(&token)
        .send()
        .await
        .assert_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["greeting"], "Hello from tests!");
}

// ─── New tests: Validation (#2) ───

#[tokio::test]
async fn test_create_user_with_valid_data() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    let body = CreateUserRequest {
        name: "Charlie".into(),
        email: "charlie@example.com".into(),
    };
    let resp = app
        .post("/users")
        .json(&body)
        .bearer(&token)
        .send()
        .await
        .assert_ok();
    let user: User = resp.json();
    assert_eq!(user.name, "Charlie");
    assert_eq!(user.email, "charlie@example.com");
}

#[tokio::test]
async fn test_create_user_with_invalid_email() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    let body = serde_json::json!({
        "name": "Valid Name",
        "email": "not-an-email"
    });
    app.post("/users")
        .json(&body)
        .bearer(&token)
        .send()
        .await
        .assert_bad_request();
}

#[tokio::test]
async fn test_create_user_with_empty_name() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    let body = serde_json::json!({
        "name": "",
        "email": "valid@example.com"
    });
    app.post("/users")
        .json(&body)
        .bearer(&token)
        .send()
        .await
        .assert_bad_request();
}

// ─── New tests: Error handling (#3) ───

#[tokio::test]
async fn test_custom_error_endpoint() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    let resp = app
        .get("/error/custom")
        .bearer(&token)
        .send()
        .await
        .assert_status(http::StatusCode::from_u16(418).unwrap());
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"], "I'm a teapot");
    assert_eq!(body["code"], 418);
}

// ─── New tests: Interceptors (#4) ───

#[tokio::test]
async fn test_cached_endpoint() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    let resp = app
        .get("/users/cached")
        .bearer(&token)
        .send()
        .await
        .assert_ok();
    let body: serde_json::Value = resp.json();
    // Should return the user list as JSON value
    assert!(body.is_array());
    assert_eq!(body.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn test_rate_limited_endpoint() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    let body = serde_json::json!({
        "name": "RateTest",
        "email": "rate@test.com"
    });

    // First 3 requests should succeed (max=3 in test controller)
    for _ in 0..3 {
        app.post("/users/rate-limited")
            .json(&body)
            .bearer(&token)
            .send()
            .await
            .assert_ok();
    }

    // 4th request should be rate limited
    app.post("/users/rate-limited")
        .json(&body)
        .bearer(&token)
        .send()
        .await
        .assert_status(http::StatusCode::TOO_MANY_REQUESTS);
}

// ─── New tests: OpenAPI (#5) ───

#[tokio::test]
async fn test_openapi_json_endpoint() {
    let (app, _jwt) = setup().await;
    let resp = app.get("/openapi.json").send().await.assert_ok();
    let spec: serde_json::Value = resp.json();
    assert_eq!(spec["openapi"], "3.0.3");
    assert_eq!(spec["info"]["title"], "Test API");
    assert!(spec["paths"].is_object());
}

#[tokio::test]
async fn test_docs_ui_endpoint() {
    let (app, _jwt) = setup().await;
    let resp = app.get("/docs").send().await.assert_ok();
    let html = resp.text();
    assert!(html.contains("wti-element"));
    assert!(html.contains("spec-url"));
}

// ─── New tests: Dev mode (#9) ───

#[tokio::test]
async fn test_dev_mode_status() {
    let (app, _jwt) = setup().await;
    let resp = app.get("/__r2e_dev/status").send().await.assert_ok();
    assert_eq!(resp.text(), "dev");
}

#[tokio::test]
async fn test_dev_mode_ping() {
    let (app, _jwt) = setup().await;
    let resp = app.get("/__r2e_dev/ping").send().await.assert_ok();
    let body: serde_json::Value = serde_json::from_str(&resp.text()).unwrap();
    assert!(body["boot_time"].is_number());
    assert_eq!(body["status"], "ok");
}

// ─── NormalizePath trailing-slash tests ───

#[tokio::test]
async fn test_trailing_slash_list_users() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    let resp = app.get("/users/").bearer(&token).send().await.assert_ok();
    let users: Vec<User> = resp.json();
    assert_eq!(users.len(), 2);
    assert_eq!(users[0].name, "Alice");
}

#[tokio::test]
async fn test_trailing_slash_get_user_by_id() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    let resp = app.get("/users/1/").bearer(&token).send().await.assert_ok();
    let user: User = resp.json();
    assert_eq!(user.name, "Alice");
}

#[tokio::test]
async fn test_trailing_slash_health() {
    let (app, _jwt) = setup().await;
    let resp = app.get("/health/").send().await.assert_ok();
    assert_eq!(resp.text(), "OK");
}

#[tokio::test]
async fn test_trailing_slash_nonexistent_still_404() {
    let (app, _jwt) = setup().await;
    app.get("/nonexistent/").send().await.assert_not_found();
}
