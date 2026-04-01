use std::sync::Arc;

use r2e::config::R2eConfig;
use r2e::prelude::*;
use r2e::r2e_security::{AuthenticatedUser, JwtClaimsValidator};
use r2e_test::{TestApp, TestJwt};

// ─── Types ───

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct User {
    pub id: u64,
    pub name: String,
}

#[derive(Clone)]
pub struct UserService {
    users: Vec<User>,
}

impl UserService {
    fn new() -> Self {
        Self {
            users: vec![
                User { id: 1, name: "Alice".into() },
                User { id: 2, name: "Bob".into() },
            ],
        }
    }

    pub fn list(&self) -> Vec<User> {
        self.users.clone()
    }
}

// ─── State ───

#[derive(Clone, TestState)]
struct MixedTestState {
    user_service: UserService,
    jwt_validator: Arc<JwtClaimsValidator>,
    config: R2eConfig,
}

// ─── Mixed controller: public + protected endpoints ───

#[derive(Controller)]
#[controller(path = "/api", state = MixedTestState)]
pub struct MixedTestController {
    #[inject]
    user_service: UserService,
}

#[routes]
impl MixedTestController {
    /// Public endpoint — no authentication required.
    #[get("/public")]
    async fn public_data(&self) -> Json<Vec<User>> {
        Json(self.user_service.list())
    }

    /// Protected endpoint — requires JWT.
    #[get("/me")]
    async fn me(
        &self,
        #[inject(identity)] user: AuthenticatedUser,
    ) -> Json<serde_json::Value> {
        Json(serde_json::json!({
            "sub": user.sub,
            "email": user.email,
        }))
    }

    /// Protected endpoint with roles.
    #[get("/admin")]
    #[roles("admin")]
    async fn admin_only(
        &self,
        #[inject(identity)] _user: AuthenticatedUser,
    ) -> Json<Vec<User>> {
        Json(self.user_service.list())
    }

    /// Optional identity — works with or without JWT.
    #[get("/whoami")]
    async fn whoami(
        &self,
        #[inject(identity)] user: Option<AuthenticatedUser>,
    ) -> Json<String> {
        match user {
            Some(u) => Json(format!("Hello, {}", u.sub)),
            None => Json("Hello, anonymous".to_string()),
        }
    }
}

async fn setup() -> (TestApp, TestJwt) {
    let jwt = TestJwt::new();
    let config = R2eConfig::empty();

    let state = MixedTestState {
        user_service: UserService::new(),
        jwt_validator: Arc::new(jwt.claims_validator()),
        config: config.clone(),
    };

    let app = TestApp::from_builder(
        AppBuilder::new()
            .with_config(config)
            .with_state(state)
            .with(ErrorHandling)
            .register_controller::<MixedTestController>(),
    );

    (app, jwt)
}

// ─── Tests ───

#[r2e::test]
async fn test_public_endpoint_no_token() {
    let (app, _jwt) = setup().await;
    let resp = app.get("/api/public").send().await;
    resp.assert_ok();
    let users: Vec<User> = resp.json();
    assert_eq!(users.len(), 2);
    assert_eq!(users[0].name, "Alice");
}

#[r2e::test]
async fn test_protected_endpoint_with_token() {
    let (app, jwt) = setup().await;
    let token = jwt.token_with_claims("user-42", &["user"], Some("test@example.com"));
    let resp = app
        .get("/api/me")
        .bearer(&token)
        .send()
        .await;
    resp.assert_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["sub"], "user-42");
    assert_eq!(body["email"], "test@example.com");
}

#[r2e::test]
async fn test_protected_endpoint_no_token() {
    let (app, _jwt) = setup().await;
    app.get("/api/me").send().await.assert_unauthorized();
}

#[r2e::test]
async fn test_optional_identity_with_token() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-42", &["user"]);
    let resp = app
        .get("/api/whoami")
        .bearer(&token)
        .send()
        .await;
    resp.assert_ok();
    let text: String = resp.json();
    assert_eq!(text, "Hello, user-42");
}

#[r2e::test]
async fn test_optional_identity_without_token() {
    let (app, _jwt) = setup().await;
    let resp = app.get("/api/whoami").send().await;
    resp.assert_ok();
    let text: String = resp.json();
    assert_eq!(text, "Hello, anonymous");
}

#[r2e::test]
async fn test_optional_identity_invalid_token() {
    let (app, _jwt) = setup().await;
    // An invalid JWT should cause an error (not treated as None)
    app.get("/api/whoami")
        .bearer("invalid.jwt.token")
        .send()
        .await
        .assert_unauthorized();
}

#[r2e::test]
async fn test_admin_endpoint_with_admin_role() {
    let (app, jwt) = setup().await;
    let token = jwt.token("admin-1", &["admin"]);
    let resp = app
        .get("/api/admin")
        .bearer(&token)
        .send()
        .await;
    resp.assert_ok();
    let users: Vec<User> = resp.json();
    assert_eq!(users.len(), 2);
}

#[r2e::test]
async fn test_admin_endpoint_without_admin_role() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    app.get("/api/admin")
        .bearer(&token)
        .send()
        .await
        .assert_forbidden();
}
