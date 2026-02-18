use axum::body::Body;
use axum::http::{Request, StatusCode};
use r2e_oidc::{ClientRegistry, InMemoryUserStore, OidcServer, OidcUser};
use tower::ServiceExt;

async fn body_json(resp: axum::http::Response<Body>) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Full integration: issue token, then validate it with the same claims_validator.
#[tokio::test]
async fn token_validates_with_claims_validator() {
    let users = InMemoryUserStore::new().add_user(
        "alice",
        "password123",
        OidcUser {
            sub: "user-1".into(),
            email: Some("alice@example.com".into()),
            roles: vec!["admin".into(), "user".into()],
            ..Default::default()
        },
    );

    let oidc = OidcServer::new()
        .issuer("http://localhost:3000")
        .audience("test-app")
        .token_ttl(7200)
        .with_user_store(users);

    let app = r2e_core::AppBuilder::new()
        .plugin(oidc)
        .with_state(())
        .build();

    // 1. Get a token.
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
    let token = json["access_token"].as_str().unwrap();
    assert_eq!(json["expires_in"], 7200);

    // 2. Use it at /userinfo.
    let req = Request::get("/userinfo")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let user_info = body_json(resp).await;
    assert_eq!(user_info["sub"], "user-1");
    assert_eq!(user_info["email"], "alice@example.com");
    assert_eq!(user_info["roles"], serde_json::json!(["admin", "user"]));
}

/// Client credentials grant.
#[tokio::test]
async fn client_credentials_grant() {
    let users = InMemoryUserStore::new().add_user(
        "alice",
        "pass",
        OidcUser {
            sub: "u1".into(),
            ..Default::default()
        },
    );

    let clients = ClientRegistry::new().add_client("my-service", "secret123");

    let oidc = OidcServer::new()
        .with_user_store(users)
        .with_client_registry(clients);

    let app = r2e_core::AppBuilder::new()
        .plugin(oidc)
        .with_state(())
        .build();

    let req = Request::builder()
        .method("POST")
        .uri("/oauth/token")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(
            "grant_type=client_credentials&client_id=my-service&client_secret=secret123",
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    assert_eq!(json["token_type"], "Bearer");
    assert!(json["access_token"].as_str().unwrap().len() > 50);
}

/// Client credentials with wrong secret.
#[tokio::test]
async fn client_credentials_invalid_secret() {
    let users = InMemoryUserStore::new().add_user(
        "alice",
        "pass",
        OidcUser {
            sub: "u1".into(),
            ..Default::default()
        },
    );

    let clients = ClientRegistry::new().add_client("my-service", "secret123");

    let oidc = OidcServer::new()
        .with_user_store(users)
        .with_client_registry(clients);

    let app = r2e_core::AppBuilder::new()
        .plugin(oidc)
        .with_state(())
        .build();

    let req = Request::builder()
        .method("POST")
        .uri("/oauth/token")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(
            "grant_type=client_credentials&client_id=my-service&client_secret=wrong",
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let json = body_json(resp).await;
    assert_eq!(json["error"], "invalid_client");
}

/// Client credentials grant when no clients are registered.
#[tokio::test]
async fn client_credentials_not_configured() {
    let users = InMemoryUserStore::new().add_user(
        "alice",
        "pass",
        OidcUser {
            sub: "u1".into(),
            ..Default::default()
        },
    );

    let oidc = OidcServer::new().with_user_store(users);

    let app = r2e_core::AppBuilder::new()
        .plugin(oidc)
        .with_state(())
        .build();

    let req = Request::builder()
        .method("POST")
        .uri("/oauth/token")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(
            "grant_type=client_credentials&client_id=svc&client_secret=sec",
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let json = body_json(resp).await;
    assert_eq!(json["error"], "unsupported_grant_type");
}

/// Base path routing.
#[tokio::test]
async fn base_path_routing() {
    let users = InMemoryUserStore::new().add_user(
        "alice",
        "password123",
        OidcUser {
            sub: "user-1".into(),
            ..Default::default()
        },
    );

    let oidc = OidcServer::new()
        .base_path("/auth")
        .with_user_store(users);

    let app = r2e_core::AppBuilder::new()
        .plugin(oidc)
        .with_state(())
        .build();

    // Token endpoint should be at /auth/oauth/token.
    let req = Request::builder()
        .method("POST")
        .uri("/auth/oauth/token")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(
            "grant_type=password&username=alice&password=password123",
        ))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // JWKS should be at /auth/.well-known/jwks.json.
    let req = Request::get("/auth/.well-known/jwks.json")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
