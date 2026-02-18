use axum::body::Body;
use axum::http::{Request, StatusCode};
use r2e_oidc::{InMemoryUserStore, OidcServer, OidcUser};
use tower::ServiceExt;

fn build_app() -> axum::Router {
    let users = InMemoryUserStore::new().add_user(
        "alice",
        "pass",
        OidcUser {
            sub: "u1".into(),
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

fn build_app_with_base_path() -> axum::Router {
    let users = InMemoryUserStore::new().add_user(
        "alice",
        "pass",
        OidcUser {
            sub: "u1".into(),
            ..Default::default()
        },
    );

    let oidc = OidcServer::new()
        .issuer("http://localhost:3000")
        .audience("test-app")
        .base_path("/auth")
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

#[tokio::test]
async fn discovery_document() {
    let app = build_app();
    let req = Request::get("/.well-known/openid-configuration")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    assert_eq!(json["issuer"], "http://localhost:3000");
    assert_eq!(
        json["token_endpoint"],
        "http://localhost:3000/oauth/token"
    );
    assert_eq!(
        json["jwks_uri"],
        "http://localhost:3000/.well-known/jwks.json"
    );
    assert_eq!(
        json["userinfo_endpoint"],
        "http://localhost:3000/userinfo"
    );
    assert!(json["grant_types_supported"].as_array().unwrap().len() == 2);
}

#[tokio::test]
async fn discovery_document_with_base_path() {
    let app = build_app_with_base_path();
    let req = Request::get("/auth/.well-known/openid-configuration")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    assert_eq!(json["issuer"], "http://localhost:3000");
    assert_eq!(
        json["token_endpoint"],
        "http://localhost:3000/auth/oauth/token"
    );
    assert_eq!(
        json["jwks_uri"],
        "http://localhost:3000/auth/.well-known/jwks.json"
    );
}
