use std::future::Future;
use std::sync::Arc;

use r2e::config::R2eConfig;
use r2e::prelude::*;
use r2e::r2e_security::{AuthenticatedUser, JwtClaimsValidator};
use r2e::{Guard, GuardContext, Identity, PreAuthGuard, PreAuthGuardContext};
use r2e_test::{TestApp, TestJwt};

// ─── Custom guard that always allows ───

pub struct AllowGuard;

impl<S: Send + Sync, I: Identity> Guard<S, I> for AllowGuard {
    fn check(
        &self,
        _state: &S,
        _ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async { Ok(()) }
    }
}

// ─── Custom guard that always rejects with 403 ───

pub struct DenyGuard;

impl<S: Send + Sync, I: Identity> Guard<S, I> for DenyGuard {
    fn check(
        &self,
        _state: &S,
        _ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async {
            Err(AppError::Forbidden("Access denied by DenyGuard".into()).into_response())
        }
    }
}

// ─── Guard that checks identity sub ───

pub struct SubCheckGuard;

impl<S: Send + Sync, I: Identity> Guard<S, I> for SubCheckGuard {
    fn check(
        &self,
        _state: &S,
        ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        let sub = ctx.identity_sub().map(|s| s.to_string());
        async move {
            match sub {
                Some(s) if s == "allowed-user" => Ok(()),
                _ => Err(AppError::Forbidden("Wrong user".into()).into_response()),
            }
        }
    }
}

// ─── Guard that checks a custom header ───

pub struct HeaderCheckGuard;

impl<S: Send + Sync, I: Identity> Guard<S, I> for HeaderCheckGuard {
    fn check(
        &self,
        _state: &S,
        ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        let has_header = ctx.headers.get("x-custom-token").is_some();
        async move {
            if has_header {
                Ok(())
            } else {
                Err(AppError::BadRequest("Missing x-custom-token header".into()).into_response())
            }
        }
    }
}

// ─── Guard that checks URI path ───

pub struct PathCheckGuard;

impl<S: Send + Sync, I: Identity> Guard<S, I> for PathCheckGuard {
    fn check(
        &self,
        _state: &S,
        ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        let path = ctx.path().to_string();
        async move {
            if path.contains("path-check") {
                Ok(())
            } else {
                Err(AppError::Forbidden("Invalid path".into()).into_response())
            }
        }
    }
}

// ─── Pre-auth guard that always rejects ───

pub struct DenyPreAuthGuard;

impl<S: Send + Sync> PreAuthGuard<S> for DenyPreAuthGuard {
    fn check(
        &self,
        _state: &S,
        _ctx: &PreAuthGuardContext<'_>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async {
            Err(AppError::Forbidden("Pre-auth denied".into()).into_response())
        }
    }
}

// ─── Pre-auth guard that checks a header ───

pub struct ApiKeyPreAuthGuard;

impl<S: Send + Sync> PreAuthGuard<S> for ApiKeyPreAuthGuard {
    fn check(
        &self,
        _state: &S,
        ctx: &PreAuthGuardContext<'_>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        let has_key = ctx
            .headers
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            .map(|v| v == "valid-key")
            .unwrap_or(false);
        async move {
            if has_key {
                Ok(())
            } else {
                Err(AppError::Unauthorized("Missing or invalid API key".into()).into_response())
            }
        }
    }
}

// ─── State ───

#[derive(Clone)]
struct GuardTestState {
    jwt_validator: Arc<JwtClaimsValidator>,
    config: R2eConfig,
}

impl r2e::http::extract::FromRef<GuardTestState> for Arc<JwtClaimsValidator> {
    fn from_ref(state: &GuardTestState) -> Self {
        state.jwt_validator.clone()
    }
}

impl r2e::http::extract::FromRef<GuardTestState> for R2eConfig {
    fn from_ref(state: &GuardTestState) -> Self {
        state.config.clone()
    }
}

// ─── Controller with various guard scenarios ───

#[derive(Controller)]
#[controller(path = "/guarded", state = GuardTestState)]
pub struct GuardTestController;

#[routes]
impl GuardTestController {
    #[get("/allow")]
    #[guard(AllowGuard)]
    async fn guarded_allow(
        &self,
        #[inject(identity)] _user: AuthenticatedUser,
    ) -> &'static str {
        "allowed"
    }

    #[get("/deny")]
    #[guard(DenyGuard)]
    async fn guarded_deny(
        &self,
        #[inject(identity)] _user: AuthenticatedUser,
    ) -> &'static str {
        "should not reach"
    }

    #[get("/sub-check")]
    #[guard(SubCheckGuard)]
    async fn guarded_sub_check(
        &self,
        #[inject(identity)] user: AuthenticatedUser,
    ) -> Json<String> {
        Json(user.sub.clone())
    }

    #[get("/header-check")]
    #[guard(HeaderCheckGuard)]
    async fn guarded_header_check(
        &self,
        #[inject(identity)] _user: AuthenticatedUser,
    ) -> &'static str {
        "header ok"
    }

    #[get("/path-check")]
    #[guard(PathCheckGuard)]
    async fn guarded_path_check(
        &self,
        #[inject(identity)] _user: AuthenticatedUser,
    ) -> &'static str {
        "path ok"
    }

    #[get("/pre-deny")]
    #[pre_guard(DenyPreAuthGuard)]
    async fn pre_guarded_deny(&self) -> &'static str {
        "should not reach"
    }

    #[get("/api-key")]
    #[pre_guard(ApiKeyPreAuthGuard)]
    async fn api_key_required(&self) -> &'static str {
        "api key ok"
    }
}

async fn setup() -> (TestApp, TestJwt) {
    let jwt = TestJwt::new();
    let config = R2eConfig::empty();

    let state = GuardTestState {
        jwt_validator: Arc::new(jwt.claims_validator()),
        config: config.clone(),
    };

    let app = TestApp::from_builder(
        AppBuilder::new()
            .with_state(state)
            .with_config(config)
            .with(ErrorHandling)
            .register_controller::<GuardTestController>(),
    );

    (app, jwt)
}

// ─── Tests ───

#[tokio::test]
async fn test_custom_guard_allows() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    let resp = app
        .get("/guarded/allow")
        .bearer(&token)
        .send()
        .await
        .assert_ok();
    assert_eq!(resp.text(), "allowed");
}

#[tokio::test]
async fn test_custom_guard_rejects() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);
    app.get("/guarded/deny")
        .bearer(&token)
        .send()
        .await
        .assert_forbidden();
}

#[tokio::test]
async fn test_guard_receives_identity() {
    let (app, jwt) = setup().await;

    // SubCheckGuard only allows "allowed-user"
    let token_allowed = jwt.token("allowed-user", &["user"]);
    let resp = app
        .get("/guarded/sub-check")
        .bearer(&token_allowed)
        .send()
        .await
        .assert_ok();
    let sub: String = resp.json();
    assert_eq!(sub, "allowed-user");

    // Other users should be rejected
    let token_denied = jwt.token("other-user", &["user"]);
    app.get("/guarded/sub-check")
        .bearer(&token_denied)
        .send()
        .await
        .assert_forbidden();
}

#[tokio::test]
async fn test_guard_receives_headers() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);

    // Without the required header → rejected
    app.get("/guarded/header-check")
        .bearer(&token)
        .send()
        .await
        .assert_bad_request();

    // With the required header → allowed
    let resp = app
        .get("/guarded/header-check")
        .bearer(&token)
        .header("x-custom-token", "anything")
        .send()
        .await
        .assert_ok();
    assert_eq!(resp.text(), "header ok");
}

#[tokio::test]
async fn test_guard_receives_uri() {
    let (app, jwt) = setup().await;
    let token = jwt.token("user-1", &["user"]);

    // PathCheckGuard allows paths starting with /guarded
    let resp = app
        .get("/guarded/path-check")
        .bearer(&token)
        .send()
        .await
        .assert_ok();
    assert_eq!(resp.text(), "path ok");
}

#[tokio::test]
async fn test_pre_guard_rejects_early() {
    let (app, _jwt) = setup().await;

    // Pre-auth guard denies without requiring any JWT
    app.get("/guarded/pre-deny")
        .send()
        .await
        .assert_forbidden();
}

#[tokio::test]
async fn test_pre_guard_api_key_required() {
    let (app, _jwt) = setup().await;

    // Without API key → unauthorized
    app.get("/guarded/api-key")
        .send()
        .await
        .assert_unauthorized();

    // With valid API key → allowed
    let resp = app
        .get("/guarded/api-key")
        .header("x-api-key", "valid-key")
        .send()
        .await
        .assert_ok();
    assert_eq!(resp.text(), "api key ok");
}

#[tokio::test]
async fn test_pre_guard_runs_before_jwt() {
    let (app, _jwt) = setup().await;

    // Even with an invalid/missing JWT, the pre-guard fires first
    // DenyPreAuthGuard returns 403, not 401 (which JWT failure would give)
    let resp = app.get("/guarded/pre-deny").send().await;
    assert_eq!(resp.status, http::StatusCode::FORBIDDEN);
}
