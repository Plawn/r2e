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

    let oidc = OidcServer::new().with_user_store(users);

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
async fn jwks_endpoint() {
    let app = build_app();
    let req = Request::get("/.well-known/jwks.json")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    let keys = json["keys"].as_array().unwrap();
    assert_eq!(keys.len(), 1);

    let key = &keys[0];
    assert_eq!(key["kty"], "RSA");
    assert_eq!(key["alg"], "RS256");
    assert_eq!(key["use"], "sig");
    assert_eq!(key["kid"], "r2e-oidc-key-1");
    assert!(key["n"].as_str().unwrap().len() > 100); // RSA-2048 modulus
    assert!(key["e"].as_str().unwrap().len() > 0);
}
