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
    use r2e::r2e_events::{EventBus, LocalEventBus};

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

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    pub struct UserCreatedEvent {
        pub user_id: u64,
        pub name: String,
        pub email: String,
    }

    #[derive(Clone)]
    pub struct UserService {
        users: Arc<RwLock<Vec<User>>>,
        event_bus: LocalEventBus,
    }

    impl UserService {
        pub fn new(event_bus: LocalEventBus) -> Self {
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
use r2e::r2e_events::LocalEventBus;

#[derive(Clone)]
struct TestServices {
    user_service: UserService,
    jwt_validator: Arc<JwtClaimsValidator>,
    pool: sqlx::SqlitePool,
    event_bus: LocalEventBus,
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

impl r2e::http::extract::FromRef<TestServices> for LocalEventBus {
    fn from_ref(state: &TestServices) -> Self {
        state.event_bus.clone()
    }
}

impl r2e::http::extract::FromRef<TestServices> for r2e::r2e_rate_limit::RateLimitRegistry {
    fn from_ref(state: &TestServices) -> Self {
        state.rate_limiter.clone()
    }
}

// ─── Test controller for OpenAPI query params ───

#[derive(serde::Deserialize, Params)]
pub struct TestSearchParams {
    #[query]
    pub name: Option<String>,
    #[query]
    pub age: Option<i64>,
}

// ─── Nested Params test types ───

#[derive(serde::Deserialize, Params)]
pub struct FlatNestedParams {
    #[params]
    pub pageable: Pageable,
    #[query]
    pub name: Option<String>,
    #[query]
    pub email: Option<String>,
}

#[derive(serde::Deserialize, Params)]
pub struct PrefixedNestedParams {
    #[params(prefix)]
    pub pageable: Pageable,
    #[query]
    pub q: Option<String>,
}

#[derive(serde::Deserialize, Params)]
pub struct CustomPrefixParams {
    #[params(prefix = "p")]
    pub pageable: Pageable,
    #[query]
    pub q: Option<String>,
}

#[derive(Controller)]
#[controller(path = "/search", state = TestServices)]
pub struct TestSearchController {
    #[inject]
    pool: sqlx::SqlitePool,
}

#[routes]
impl TestSearchController {
    #[get("/")]
    async fn search(&self, Query(_params): Query<TestSearchParams>) -> Json<Vec<User>> {
        Json(vec![])
    }

    #[get("/paged")]
    async fn paged(&self, Query(_pageable): Query<Pageable>) -> Json<Vec<User>> {
        Json(vec![])
    }
}

// ─── Nested Params test controller ───

#[derive(Controller)]
#[controller(path = "/nested", state = TestServices)]
pub struct NestedParamsController {
    #[inject]
    pool: sqlx::SqlitePool,
}

#[routes]
impl NestedParamsController {
    #[get("/flat")]
    async fn flat_search(&self, params: FlatNestedParams) -> Json<serde_json::Value> {
        Json(serde_json::json!({
            "page": params.pageable.page,
            "size": params.pageable.size,
            "sort": params.pageable.sort,
            "name": params.name,
            "email": params.email,
        }))
    }

    #[get("/prefixed")]
    async fn prefixed_search(&self, params: PrefixedNestedParams) -> Json<serde_json::Value> {
        Json(serde_json::json!({
            "page": params.pageable.page,
            "size": params.pageable.size,
            "sort": params.pageable.sort,
            "q": params.q,
        }))
    }

    #[get("/custom-prefix")]
    async fn custom_prefix_search(&self, params: CustomPrefixParams) -> Json<serde_json::Value> {
        Json(serde_json::json!({
            "page": params.pageable.page,
            "size": params.pageable.size,
            "sort": params.pageable.sort,
            "q": params.q,
        }))
    }
}

// ─── Main test controller ───

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
    ) -> Result<Json<User>, HttpError> {
        match self.user_service.get_by_id(id).await {
            Some(user) => Ok(Json(user)),
            None => Err(HttpError::NotFound("User not found".into())),
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
    async fn custom_error(&self) -> Result<Json<()>, HttpError> {
        Err(HttpError::Custom {
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
    ) -> Result<Json<User>, HttpError> {
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
    let event_bus = LocalEventBus::new();

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
            .register_controller::<TestUserController>()
            .register_controller::<TestSearchController>()
            .register_controller::<NestedParamsController>(),
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
    assert_eq!(spec["openapi"], "3.1.0");
    assert_eq!(spec["info"]["title"], "Test API");
    assert!(spec["paths"].is_object());
}

#[tokio::test]
async fn test_openapi_path_params() {
    let (app, _jwt) = setup().await;
    let spec: serde_json::Value = app.get("/openapi.json").send().await.assert_ok().json();

    let params = spec["paths"]["/users/{id}"]["get"]["parameters"]
        .as_array()
        .expect("GET /users/{id} should have parameters");
    assert!(
        params.iter().any(|p| p["name"] == "id" && p["in"] == "path" && p["required"] == true),
        "should have required path param 'id'"
    );
}

#[tokio::test]
async fn test_openapi_query_params_from_derive() {
    let (app, _jwt) = setup().await;
    let spec: serde_json::Value = app.get("/openapi.json").send().await.assert_ok().json();

    let params = spec["paths"]["/search/"]["get"]["parameters"]
        .as_array()
        .expect("GET /search/ should have parameters from #[derive(Params)]");

    assert_eq!(params.len(), 2, "search endpoint should have 2 query params");

    let name_param = params.iter().find(|p| p["name"] == "name").expect("missing 'name' param");
    assert_eq!(name_param["in"], "query");
    assert_eq!(name_param["required"], false);
    assert_eq!(name_param["schema"]["type"], "string");

    let age_param = params.iter().find(|p| p["name"] == "age").expect("missing 'age' param");
    assert_eq!(age_param["in"], "query");
    assert_eq!(age_param["required"], false);
    assert_eq!(age_param["schema"]["type"], "integer");
}

#[tokio::test]
async fn test_openapi_pageable_params() {
    let (app, _jwt) = setup().await;
    let spec: serde_json::Value = app.get("/openapi.json").send().await.assert_ok().json();

    let params = spec["paths"]["/search/paged"]["get"]["parameters"]
        .as_array()
        .expect("GET /search/paged should have Pageable parameters");

    assert_eq!(params.len(), 3, "Pageable should expose page, size, sort");

    assert!(params.iter().any(|p| p["name"] == "page" && p["schema"]["type"] == "integer"));
    assert!(params.iter().any(|p| p["name"] == "size" && p["schema"]["type"] == "integer"));
    assert!(params.iter().any(|p| p["name"] == "sort" && p["schema"]["type"] == "string"));

    // All Pageable params are optional (they have defaults)
    for p in params {
        assert_eq!(p["required"], false, "Pageable param '{}' should be optional", p["name"]);
    }
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

// ─── Nested Params extraction tests ───

#[tokio::test]
async fn test_flat_nested_params_extraction() {
    let (app, _jwt) = setup().await;
    let resp = app
        .get("/nested/flat?page=2&size=10&sort=name&name=alice&email=a@b.com")
        .send()
        .await
        .assert_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["page"], 2);
    assert_eq!(body["size"], 10);
    assert_eq!(body["sort"], "name");
    assert_eq!(body["name"], "alice");
    assert_eq!(body["email"], "a@b.com");
}

#[tokio::test]
async fn test_flat_nested_params_defaults() {
    let (app, _jwt) = setup().await;
    // No page/size → Pageable defaults (page=0, size=20)
    let resp = app
        .get("/nested/flat?name=bob")
        .send()
        .await
        .assert_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["page"], 0);
    assert_eq!(body["size"], 20);
    assert_eq!(body["name"], "bob");
    assert!(body["sort"].is_null());
    assert!(body["email"].is_null());
}

#[tokio::test]
async fn test_prefixed_nested_params_extraction() {
    let (app, _jwt) = setup().await;
    let resp = app
        .get("/nested/prefixed?pageable.page=3&pageable.size=5&pageable.sort=id&q=hello")
        .send()
        .await
        .assert_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["page"], 3);
    assert_eq!(body["size"], 5);
    assert_eq!(body["sort"], "id");
    assert_eq!(body["q"], "hello");
}

#[tokio::test]
async fn test_prefixed_nested_params_defaults() {
    let (app, _jwt) = setup().await;
    // Pageable fields without prefix should NOT populate the prefixed params
    let resp = app
        .get("/nested/prefixed?page=99&q=test")
        .send()
        .await
        .assert_ok();
    let body: serde_json::Value = resp.json();
    // page=99 should NOT be picked up because the prefix is "pageable"
    assert_eq!(body["page"], 0);
    assert_eq!(body["size"], 20);
    assert_eq!(body["q"], "test");
}

#[tokio::test]
async fn test_custom_prefix_nested_params_extraction() {
    let (app, _jwt) = setup().await;
    let resp = app
        .get("/nested/custom-prefix?p.page=1&p.size=50&q=world")
        .send()
        .await
        .assert_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["page"], 1);
    assert_eq!(body["size"], 50);
    assert_eq!(body["q"], "world");
}

// ─── Nested Params OpenAPI tests ───

#[tokio::test]
async fn test_openapi_flat_nested_params() {
    let (app, _jwt) = setup().await;
    let spec: serde_json::Value = app.get("/openapi.json").send().await.assert_ok().json();

    let params = spec["paths"]["/nested/flat"]["get"]["parameters"]
        .as_array()
        .expect("GET /nested/flat should have parameters");

    // Should have: page, size, sort (from Pageable), name, email (own)
    assert_eq!(params.len(), 5, "flat nested should have 5 params, got: {:?}", params);

    assert!(params.iter().any(|p| p["name"] == "page" && p["in"] == "query"));
    assert!(params.iter().any(|p| p["name"] == "size" && p["in"] == "query"));
    assert!(params.iter().any(|p| p["name"] == "sort" && p["in"] == "query"));
    assert!(params.iter().any(|p| p["name"] == "name" && p["in"] == "query"));
    assert!(params.iter().any(|p| p["name"] == "email" && p["in"] == "query"));
}

#[tokio::test]
async fn test_openapi_prefixed_nested_params() {
    let (app, _jwt) = setup().await;
    let spec: serde_json::Value = app.get("/openapi.json").send().await.assert_ok().json();

    let params = spec["paths"]["/nested/prefixed"]["get"]["parameters"]
        .as_array()
        .expect("GET /nested/prefixed should have parameters");

    // Should have: pageable.page, pageable.size, pageable.sort, q
    assert_eq!(params.len(), 4, "prefixed nested should have 4 params, got: {:?}", params);

    assert!(params.iter().any(|p| p["name"] == "pageable.page" && p["in"] == "query"));
    assert!(params.iter().any(|p| p["name"] == "pageable.size" && p["in"] == "query"));
    assert!(params.iter().any(|p| p["name"] == "pageable.sort" && p["in"] == "query"));
    assert!(params.iter().any(|p| p["name"] == "q" && p["in"] == "query"));
}

#[tokio::test]
async fn test_openapi_custom_prefix_params() {
    let (app, _jwt) = setup().await;
    let spec: serde_json::Value = app.get("/openapi.json").send().await.assert_ok().json();

    let params = spec["paths"]["/nested/custom-prefix"]["get"]["parameters"]
        .as_array()
        .expect("GET /nested/custom-prefix should have parameters");

    assert_eq!(params.len(), 4, "custom prefix should have 4 params, got: {:?}", params);

    assert!(params.iter().any(|p| p["name"] == "p.page" && p["in"] == "query"));
    assert!(params.iter().any(|p| p["name"] == "p.size" && p["in"] == "query"));
    assert!(params.iter().any(|p| p["name"] == "p.sort" && p["in"] == "query"));
    assert!(params.iter().any(|p| p["name"] == "q" && p["in"] == "query"));
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
