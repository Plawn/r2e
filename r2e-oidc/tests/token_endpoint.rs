use axum::body::Body;
use axum::http::{Request, StatusCode};
use r2e_oidc::{InMemoryUserStore, OidcServer, OidcUser};
use tower::ServiceExt;

fn build_app() -> axum::Router {
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
        .with_user_store(users);

    r2e_core::AppBuilder::new()
        .plugin(oidc)
        .with_state(())
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

async fn body_json(resp: axum::http::Response<Body>) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn password_grant_success() {
    let app = build_app();
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

#[tokio::test]
async fn password_grant_invalid_password() {
    let app = build_app();
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

#[tokio::test]
async fn password_grant_unknown_user() {
    let app = build_app();
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

#[tokio::test]
async fn unsupported_grant_type() {
    let app = build_app();
    let resp = app
        .oneshot(token_request(
            "grant_type=authorization_code&code=abc",
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert_eq!(json["error"], "unsupported_grant_type");
}

#[tokio::test]
async fn missing_username() {
    let app = build_app();
    let resp = app
        .oneshot(token_request("grant_type=password&password=abc"))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert_eq!(json["error"], "invalid_request");
}

#[tokio::test]
async fn missing_password() {
    let app = build_app();
    let resp = app
        .oneshot(token_request("grant_type=password&username=alice"))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert_eq!(json["error"], "invalid_request");
}
