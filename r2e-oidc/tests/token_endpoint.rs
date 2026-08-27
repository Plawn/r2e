use r2e_core::http::body::to_bytes;
use r2e_core::http::{Body, Request, Response, Router, StatusCode};
use r2e_oidc::{ClientRegistry, InMemoryUserStore, OidcServer, OidcUser};
use tower::ServiceExt;

async fn build_app() -> Router {
    let users = InMemoryUserStore::new()
        .add_user(
            "alice",
            "password123",
            OidcUser {
                sub: "user-1".into(),
                email: Some("alice@example.com".into()),
                roles: vec!["admin".into()],
                ..Default::default()
            },
        )
        .add_user(
            "bob",
            "password456",
            OidcUser {
                sub: "user-2".into(),
                roles: vec!["user".into()],
                ..Default::default()
            },
        );

    let oidc = OidcServer::new()
        .issuer("http://localhost:3000")
        .audience("test-app")
        .enable_password_grant_for_development()
        .with_user_store(users);

    r2e_core::AppBuilder::new()
        .plugin(oidc)
        .build_state()
        .await
        .build()
}

fn token_request(body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/oauth/token")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn body_json(resp: Response) -> serde_json::Value {
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[r2e_core::test]
async fn password_grant_success() {
    let app = build_app().await;
    let resp = app
        .oneshot(token_request(
            "grant_type=password&username=alice&password=password123",
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["token_type"], "Bearer");
    assert_eq!(json["expires_in"], 3600);
    assert!(json["access_token"].as_str().unwrap().len() > 50);
}

#[r2e_core::test]
async fn password_grant_invalid_password() {
    let app = build_app().await;
    let resp = app
        .oneshot(token_request(
            "grant_type=password&username=alice&password=wrong",
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert_eq!(json["error"], "invalid_grant");
}

#[r2e_core::test]
async fn password_grant_unknown_user() {
    let app = build_app().await;
    let resp = app
        .oneshot(token_request(
            "grant_type=password&username=nobody&password=pass",
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert_eq!(json["error"], "invalid_grant");
}

#[r2e_core::test]
async fn unsupported_grant_type() {
    let app = build_app().await;
    let resp = app
        .oneshot(token_request("grant_type=authorization_code&code=abc"))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert_eq!(json["error"], "unsupported_grant_type");
}

#[r2e_core::test]
async fn missing_grant_type() {
    let app = build_app().await;
    let resp = app
        .oneshot(token_request("username=alice&password=password123"))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert_eq!(json["error"], "invalid_request");
}

#[r2e_core::test]
async fn missing_username() {
    let app = build_app().await;
    let resp = app
        .oneshot(token_request("grant_type=password&password=abc"))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert_eq!(json["error"], "invalid_request");
}

#[r2e_core::test]
async fn missing_password() {
    let app = build_app().await;
    let resp = app
        .oneshot(token_request("grant_type=password&username=alice"))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert_eq!(json["error"], "invalid_request");
}

#[r2e_core::test]
async fn password_grant_disabled_by_default() {
    let users = InMemoryUserStore::new().add_user(
        "alice",
        "password123",
        OidcUser {
            sub: "user-1".into(),
            ..Default::default()
        },
    );

    let app = r2e_core::AppBuilder::new()
        .plugin(OidcServer::new().with_user_store(users))
        .build_state()
        .await
        .build();

    let resp = app
        .oneshot(token_request(
            "grant_type=password&username=alice&password=password123",
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert_eq!(json["error"], "unsupported_grant_type");
}

// ── RFC 8707 `resource` indicator ──────────────────────────────────────────

/// Decode the (unverified) JWT payload — enough to observe issued claims.
fn jwt_payload(token: &str) -> serde_json::Value {
    use base64::Engine;
    let part = token.split('.').nth(1).unwrap();
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(part)
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[r2e_core::test]
async fn resource_indicator_becomes_the_audience() {
    let app = build_app().await;
    let resp = app
        .oneshot(token_request(
            "grant_type=password&username=alice&password=password123\
             &resource=http%3A%2F%2Flocalhost%3A3000%2Fmcp",
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let claims = jwt_payload(json["access_token"].as_str().unwrap());
    assert_eq!(claims["aud"], "http://localhost:3000/mcp");
}

#[r2e_core::test]
async fn no_resource_keeps_the_configured_audience() {
    let app = build_app().await;
    let resp = app
        .oneshot(token_request(
            "grant_type=password&username=alice&password=password123",
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let claims = jwt_payload(json["access_token"].as_str().unwrap());
    assert_eq!(claims["aud"], "test-app");
}

#[r2e_core::test]
async fn relative_resource_is_invalid_target() {
    let app = build_app().await;
    let resp = app
        .oneshot(token_request(
            "grant_type=password&username=alice&password=password123&resource=mcp",
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert_eq!(json["error"], "invalid_target");
}

#[r2e_core::test]
async fn fragment_bearing_resource_is_invalid_target() {
    let app = build_app().await;
    let resp = app
        .oneshot(token_request(
            "grant_type=password&username=alice&password=password123\
             &resource=http%3A%2F%2Flocalhost%2Fmcp%23frag",
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert_eq!(json["error"], "invalid_target");
}

#[r2e_core::test]
async fn client_credentials_honors_the_resource_indicator() {
    let clients = ClientRegistry::new().add_client("svc", "secret");
    let app = r2e_core::AppBuilder::new()
        .plugin(
            OidcServer::new()
                .issuer("http://localhost:3000")
                .audience("test-app")
                .with_user_store(InMemoryUserStore::new())
                .with_client_registry(clients),
        )
        .build_state()
        .await
        .build();

    let resp = app
        .oneshot(token_request(
            "grant_type=client_credentials&client_id=svc&client_secret=secret\
             &resource=https%3A%2F%2Fapi.example.com%2Fmcp",
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let claims = jwt_payload(json["access_token"].as_str().unwrap());
    assert_eq!(claims["aud"], "https://api.example.com/mcp");
}
