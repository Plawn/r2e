use axum::body::Body;
use axum::http::{Request, StatusCode};
use r2e_oidc::{InMemoryUserStore, OidcServer, OidcUser};
use tower::ServiceExt;

fn build_app() -> axum::Router {
    let users = InMemoryUserStore::new().add_user(
        "alice",
        "password123",
        OidcUser {
            sub: "user-1".into(),
            email: Some("alice@example.com".into()),
            roles: vec!["admin".into()],
            ..Default::default()
        },
    );

    let oidc = OidcServer::new()
        .issuer("http://localhost:3000")
        .audience("test-app")
        .with_user_store(users);

    r2e_core::AppBuilder::new()
        .plugin(oidc)
        .with_state(())
        .build()
}

async fn body_json(resp: axum::http::Response<Body>) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn get_token(app: &axum::Router) -> String {
    let req = Request::builder()
        .method("POST")
        .uri("/oauth/token")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(
            "grant_type=password&username=alice&password=password123",
        ))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    json["access_token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn userinfo_success() {
    let app = build_app();
    let token = get_token(&app).await;

    let req = Request::get("/userinfo")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    assert_eq!(json["sub"], "user-1");
    assert_eq!(json["email"], "alice@example.com");
    assert_eq!(json["roles"], serde_json::json!(["admin"]));
}

#[tokio::test]
async fn userinfo_missing_auth_header() {
    let app = build_app();

    let req = Request::get("/userinfo").body(Body::empty()).unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn userinfo_invalid_token() {
    let app = build_app();

    let req = Request::get("/userinfo")
        .header("authorization", "Bearer invalid.token.here")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
