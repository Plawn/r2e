use axum::body::Body;
use axum::http::{Request, StatusCode};
use r2e_oidc::{InMemoryUserStore, OidcServer, OidcUser};
use tower::ServiceExt;

async fn body_json(resp: axum::http::Response<Body>) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Simulate hot-reload: build the OidcRuntime once, then use it across two
/// separate AppBuilder cycles. A token issued during the first cycle must
/// remain valid in the second cycle (same keys, same validator).
#[tokio::test]
async fn token_survives_hot_reload() {
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

    // Build the runtime once (simulates setup()).
    let oidc = OidcServer::new()
        .issuer("http://localhost:3000")
        .audience("test-app")
        .with_user_store(users)
        .build();

    // --- First hot-patch cycle ---
    let app1 = r2e_core::AppBuilder::new()
        .plugin(oidc.clone())
        .with_state(())
        .build();

    let req = Request::builder()
        .method("POST")
        .uri("/oauth/token")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(
            "grant_type=password&username=alice&password=password123",
        ))
        .unwrap();

    let resp = app1.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    let token = json["access_token"].as_str().unwrap().to_string();

    // --- Second hot-patch cycle (same runtime, new AppBuilder) ---
    let app2 = r2e_core::AppBuilder::new()
        .plugin(oidc.clone())
        .with_state(())
        .build();

    // The token from the first cycle must still be accepted.
    let req = Request::get("/userinfo")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();

    let resp = app2.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let user_info = body_json(resp).await;
    assert_eq!(user_info["sub"], "user-1");
    assert_eq!(user_info["email"], "alice@example.com");
}
