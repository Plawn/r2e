//! Naming a tenant on a test request: `as_tenant` and `as_tenant_user`.
//!
//! The echo controller reports exactly what reached the server — the tenant
//! header and the raw `Authorization` value — so the assertions are about the
//! wire, not about the builder's internals.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use r2e_core::http::HeaderMap;
use r2e_core::prelude::*;
use r2e_core::AppBuilder;
use r2e_test::{TestApp, TestJwt, TENANT_CLAIM, TENANT_HEADER};
use serde_json::Value;

#[controller(path = "/echo")]
struct EchoController;

#[routes]
impl EchoController {
    /// `<tenant header>|<authorization header>`, with `-` for an absent one.
    #[get("/")]
    async fn echo(&self, headers: HeaderMap) -> String {
        let header = |name: &str| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("-")
                .to_string()
        };
        format!("{}|{}", header(TENANT_HEADER), header("authorization"))
    }
}

async fn app() -> TestApp {
    let builder = AppBuilder::new()
        .build_state()
        .await
        .register_controller::<EchoController>();
    TestApp::new(builder.build()).with_jwt(TestJwt::new())
}

/// The claims of the Bearer token in an echoed `<tenant>|<authorization>` body.
fn claims_of(body: &str) -> Value {
    let auth = body.split('|').nth(1).expect("echoed authorization header");
    let token = auth.strip_prefix("Bearer ").expect("a Bearer token");
    let payload = token.split('.').nth(1).expect("a JWT payload segment");
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap()
}

#[tokio::test]
async fn as_tenant_sets_the_tenant_header() {
    let app = app().await;
    let body = app.get("/echo").as_tenant("acme").send().await.text();
    assert_eq!(body, "acme|-", "the tenant header, and no authentication");
}

#[tokio::test]
async fn as_tenant_user_authenticates_and_names_the_tenant() {
    let app = app().await;
    let body = app
        .get("/echo")
        .as_tenant_user("alice", "acme", &["admin"])
        .send()
        .await
        .text();

    assert!(body.starts_with("acme|Bearer "), "{body}");
    let claims = claims_of(&body);
    assert_eq!(claims["sub"], "alice");
    assert_eq!(claims[TENANT_CLAIM], "acme");
    assert_eq!(claims["roles"], serde_json::json!(["admin"]));
}

#[tokio::test]
async fn a_session_can_name_the_tenant_per_request() {
    let app = app().await;
    let session = app.session();

    let body = session.get("/echo").as_tenant("globex").send().await.text();
    assert_eq!(body, "globex|-");

    let body = session
        .get("/echo")
        .as_tenant_user("bob", "globex", &[])
        .send()
        .await
        .text();
    assert!(body.starts_with("globex|Bearer "), "{body}");
    let claims = claims_of(&body);
    assert_eq!(claims["sub"], "bob");
    assert_eq!(claims[TENANT_CLAIM], "globex");
}
