use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use r2e_core::http::body::to_bytes;
use r2e_core::http::{header, Body, Request, Response, StatusCode};
use r2e_oidc::{ClientRegistry, InMemoryUserStore, OidcServer, OidcUser};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

async fn body_json(resp: Response) -> serde_json::Value {
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn form(pairs: &[(&str, &str)]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.extend_pairs(pairs.iter().copied());
    serializer.finish()
}

#[r2e_core::test]
async fn authorization_code_pkce_is_discoverable_one_time_and_bound() {
    const AUDIENCE: &str = "http://localhost:3000/mcp";
    const REDIRECT: &str = "http://127.0.0.1:49152/callback";
    const VERIFIER: &str = "0123456789abcdefghijklmnopqrstuvwxyz-._~ABCDE";

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
    let clients = ClientRegistry::new().add_public_client("mcp-client", [REDIRECT]);
    let app = r2e_core::AppBuilder::new()
        .plugin(
            OidcServer::new()
                .audience(AUDIENCE)
                .with_user_store(users)
                .with_client_registry(clients),
        )
        .build_state()
        .await
        .build();

    let discovery = app
        .clone()
        .oneshot(
            Request::get("/.well-known/openid-configuration")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let discovery = body_json(discovery).await;
    assert_eq!(
        discovery["authorization_endpoint"],
        "http://localhost:3000/oauth/authorize"
    );
    assert!(discovery["grant_types_supported"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("authorization_code")));
    assert_eq!(
        discovery["code_challenge_methods_supported"],
        serde_json::json!(["S256"])
    );

    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes()));
    let authorize = form(&[
        ("response_type", "code"),
        ("client_id", "mcp-client"),
        ("redirect_uri", REDIRECT),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
        ("scope", "openid mcp:read"),
        ("state", "state-123"),
        ("resource", AUDIENCE),
    ]);
    let login = app
        .clone()
        .oneshot(
            Request::get(format!("/oauth/authorize?{authorize}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    assert_eq!(login.headers()[header::CACHE_CONTROL], "no-store");

    let submit = form(&[
        ("response_type", "code"),
        ("client_id", "mcp-client"),
        ("redirect_uri", REDIRECT),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
        ("scope", "openid mcp:read"),
        ("state", "state-123"),
        ("resource", AUDIENCE),
        ("username", "alice"),
        ("password", "password123"),
    ]);
    let redirect = app
        .clone()
        .oneshot(
            Request::post("/oauth/authorize")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(submit))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(redirect.status().is_redirection());
    let location = redirect.headers()[header::LOCATION].to_str().unwrap();
    let location = url::Url::parse(location).unwrap();
    assert_eq!(
        location.origin().ascii_serialization(),
        "http://127.0.0.1:49152"
    );
    let params: std::collections::HashMap<_, _> = location.query_pairs().into_owned().collect();
    assert_eq!(params["state"], "state-123");
    let code = &params["code"];

    let redeem = form(&[
        ("grant_type", "authorization_code"),
        ("client_id", "mcp-client"),
        ("redirect_uri", REDIRECT),
        ("code", code),
        ("code_verifier", VERIFIER),
    ]);
    let token = app
        .clone()
        .oneshot(
            Request::post("/oauth/token")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(redeem.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(token.status(), StatusCode::OK);
    let token = body_json(token).await;
    assert!(token["access_token"].as_str().unwrap().len() > 50);

    let replay = app
        .oneshot(
            Request::post("/oauth/token")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(redeem))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_json(replay).await["error"], "invalid_grant");
}

/// Full integration: issue token, then validate it with the same claims_validator.
#[r2e_core::test]
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
        .enable_password_grant_for_development()
        .with_user_store(users);

    let app = r2e_core::AppBuilder::new()
        .plugin(oidc)
        .build_state()
        .await
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
#[r2e_core::test]
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
        .build_state()
        .await
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

/// Client credentials support HTTP Basic authentication.
#[r2e_core::test]
async fn client_credentials_grant_with_basic_auth() {
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
        .build_state()
        .await
        .build();

    let req = Request::builder()
        .method("POST")
        .uri("/oauth/token")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", "Basic bXktc2VydmljZTpzZWNyZXQxMjM=")
        .body(Body::from("grant_type=client_credentials"))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    assert_eq!(json["token_type"], "Bearer");
}

/// Machine tokens must not be accepted by /userinfo, even if client_id collides with a user sub.
#[r2e_core::test]
async fn userinfo_rejects_client_credentials_token() {
    let users = InMemoryUserStore::new().add_user(
        "alice",
        "pass",
        OidcUser {
            sub: "my-service".into(),
            email: Some("alice@example.com".into()),
            ..Default::default()
        },
    );

    let clients = ClientRegistry::new().add_client("my-service", "secret123");
    let oidc = OidcServer::new()
        .with_user_store(users)
        .with_client_registry(clients);
    let app = r2e_core::AppBuilder::new()
        .plugin(oidc)
        .build_state()
        .await
        .build();

    let req = Request::builder()
        .method("POST")
        .uri("/oauth/token")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(
            "grant_type=client_credentials&client_id=my-service&client_secret=secret123",
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let token = json["access_token"].as_str().unwrap();

    let req = Request::get("/userinfo")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// Client credentials with wrong secret.
#[r2e_core::test]
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
        .build_state()
        .await
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
#[r2e_core::test]
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
        .build_state()
        .await
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
#[r2e_core::test]
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
        .enable_password_grant_for_development()
        .with_user_store(users);

    let app = r2e_core::AppBuilder::new()
        .plugin(oidc)
        .build_state()
        .await
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

/// Pinning the ONE bean the OIDC plugin provides must not unmount its routes.
///
/// `Arc<JwtClaimsValidator>` is the whole of `OidcServer::Provided`, and every
/// harness that stubs authentication pins exactly that (`TestApp` does). The
/// plugin's real output — `/oauth/token`, discovery, JWKS, `/userinfo` — is a
/// build **effect**, which no bean pin can stand in for. Hence
/// `SKIP_BUILD_WHEN_ALL_PINNED` defaults to `false`: OIDC never opts in.
#[r2e_core::test]
async fn pinning_the_validator_keeps_the_oidc_routes() {
    use r2e_core::type_list::BeanAccess;
    use r2e_security::{JwtClaimsValidator, SecurityConfig};
    use std::sync::Arc;

    let mock = Arc::new(JwtClaimsValidator::new_with_static_key(
        jsonwebtoken::DecodingKey::from_secret(b"mock-key"),
        SecurityConfig::new("local", "http://mock", "mock-app"),
    ));

    let users = InMemoryUserStore::new().add_user(
        "alice",
        "password123",
        OidcUser {
            sub: "user-1".into(),
            ..Default::default()
        },
    );
    let oidc = OidcServer::new()
        .enable_password_grant_for_development()
        .with_user_store(users);

    let app = r2e_core::AppBuilder::new()
        .override_bean(Arc::clone(&mock))
        .plugin(oidc)
        .build_state()
        .await;

    // The pin wins for the bean itself…
    assert!(
        Arc::ptr_eq(&app.state().get::<Arc<JwtClaimsValidator>>(), &mock),
        "the pinned validator must win over the plugin's own"
    );

    // …and the routes are still there: the build ran despite the full pin.
    let router = app.build();
    let req = Request::builder()
        .method("POST")
        .uri("/oauth/token")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(
            "grant_type=password&username=alice&password=password123",
        ))
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "/oauth/token must still be mounted when the validator is pinned"
    );

    let resp = router
        .oneshot(
            Request::get("/.well-known/openid-configuration")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "discovery still mounted");
}
