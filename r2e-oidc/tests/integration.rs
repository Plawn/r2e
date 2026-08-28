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

async fn body_text(resp: Response) -> String {
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// Pull the one-time CSRF token out of a rendered login page.
fn csrf_token(html: &str) -> String {
    let marker = "name=\"csrf_token\" value=\"";
    let start = html
        .find(marker)
        .expect("login page must embed a CSRF token")
        + marker.len();
    let rest = &html[start..];
    rest[..rest.find('"').expect("unterminated csrf token")].to_string()
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
    let clients = ClientRegistry::new()
        .add_public_client("mcp-client", [REDIRECT])
        .with_scopes(["openid", "mcp:read"]);
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
    let csrf = csrf_token(&body_text(login).await);

    let submit = form(&[
        ("response_type", "code"),
        ("client_id", "mcp-client"),
        ("redirect_uri", REDIRECT),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
        ("scope", "openid mcp:read"),
        ("state", "state-123"),
        ("resource", AUDIENCE),
        ("csrf_token", &csrf),
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
    // RFC 6749 §5.1 hygiene: the Location header carries the code.
    assert_eq!(redirect.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(redirect.headers()[header::PRAGMA], "no-cache");
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

// ── Authorization endpoint error handling (RFC 6749 §4.1.2.1) ──────────────

const AUTH_AUDIENCE: &str = "http://localhost:3000/mcp";
const AUTH_REDIRECT: &str = "http://127.0.0.1:49152/callback";
const AUTH_VERIFIER: &str = "0123456789abcdefghijklmnopqrstuvwxyz-._~ABCDE";

async fn authorize_app() -> r2e_core::http::Router {
    let users = InMemoryUserStore::new().add_user(
        "alice",
        "password123",
        OidcUser {
            sub: "user-1".into(),
            ..Default::default()
        },
    );
    let clients = ClientRegistry::new()
        .add_public_client("mcp-client", [AUTH_REDIRECT])
        .with_scopes(["openid", "mcp:read"]);
    r2e_core::AppBuilder::new()
        .plugin(
            OidcServer::new()
                .audience(AUTH_AUDIENCE)
                .with_user_store(users)
                .with_client_registry(clients),
        )
        .build_state()
        .await
        .build()
}

fn challenge() -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(AUTH_VERIFIER.as_bytes()))
}

/// Build authorize parameters; `None` removes a parameter, `Some` replaces it.
fn authorize_params<'a>(
    challenge: &'a str,
    overrides: &[(&'a str, Option<&'a str>)],
) -> Vec<(&'a str, &'a str)> {
    let mut params = vec![
        ("response_type", "code"),
        ("client_id", "mcp-client"),
        ("redirect_uri", AUTH_REDIRECT),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("scope", "openid"),
        ("state", "state-123"),
    ];
    for (name, value) in overrides {
        params.retain(|(existing, _)| existing != name);
        if let Some(value) = value {
            params.push((name, value));
        }
    }
    params
}

fn redirected_error(resp: &Response) -> (String, Option<String>) {
    let location = resp.headers()[header::LOCATION].to_str().unwrap();
    let location = url::Url::parse(location).unwrap();
    assert_eq!(
        location.origin().ascii_serialization(),
        "http://127.0.0.1:49152",
        "errors must be reported to the registered redirect URI"
    );
    let params: std::collections::HashMap<_, _> = location.query_pairs().into_owned().collect();
    assert!(
        params.contains_key("error_description"),
        "an error redirect must describe the failure"
    );
    (
        params["error"].clone(),
        params.get("state").map(ToString::to_string),
    )
}

#[r2e_core::test]
async fn authorize_errors_redirect_to_the_registered_client() {
    let app = authorize_app().await;
    let challenge = challenge();
    let cases: [(&str, Vec<(&str, Option<&str>)>); 5] = [
        (
            "unsupported_response_type",
            vec![("response_type", Some("token"))],
        ),
        ("invalid_request", vec![("code_challenge", None)]),
        (
            "invalid_request",
            vec![("code_challenge_method", Some("plain"))],
        ),
        (
            "invalid_target",
            vec![("resource", Some("https://evil.example/mcp"))],
        ),
        ("invalid_scope", vec![("scope", Some("openid mcp:admin"))]),
    ];

    for (expected, overrides) in cases {
        let query = form(&authorize_params(&challenge, &overrides));
        let resp = app
            .clone()
            .oneshot(
                Request::get(format!("/oauth/authorize?{query}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::SEE_OTHER,
            "{expected} must be reported by redirecting"
        );
        assert_eq!(resp.headers()[header::CACHE_CONTROL], "no-store");
        let (error, state) = redirected_error(&resp);
        assert_eq!(error, expected);
        assert_eq!(state.as_deref(), Some("state-123"), "state must be echoed");
    }
}

#[r2e_core::test]
async fn authorize_never_redirects_to_an_unvalidated_client() {
    let app = authorize_app().await;
    let challenge = challenge();
    let cases = [
        vec![("client_id", Some("unknown-client"))],
        vec![("redirect_uri", Some("http://127.0.0.1:49152/evil"))],
        vec![("client_id", None)],
        vec![("redirect_uri", None)],
    ];

    for overrides in cases {
        // A protocol error is present too: it must NOT turn into a redirect.
        let mut overrides = overrides;
        overrides.push(("response_type", Some("token")));
        let query = form(&authorize_params(&challenge, &overrides));
        let resp = app
            .clone()
            .oneshot(
                Request::get(format!("/oauth/authorize?{query}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(
            resp.headers().get(header::LOCATION).is_none(),
            "an unvalidated client must never receive a redirect"
        );
        assert_eq!(body_json(resp).await["error"], "invalid_request");
    }
}

/// GET the login page and return `(csrf_token, query)` for the POST that follows.
async fn login_page(app: &r2e_core::http::Router, query: &str) -> String {
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/oauth/authorize?{query}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    csrf_token(&body_text(resp).await)
}

async fn post_login(app: &r2e_core::http::Router, body: String) -> Response {
    app.clone()
        .oneshot(
            Request::post("/oauth/authorize")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[r2e_core::test]
async fn bad_credentials_redirect_with_access_denied() {
    let app = authorize_app().await;
    let challenge = challenge();
    let params = authorize_params(&challenge, &[]);
    let csrf = login_page(&app, &form(&params)).await;

    let mut submit = params.clone();
    submit.push(("csrf_token", &csrf));
    submit.push(("username", "alice"));
    submit.push(("password", "wrong-password"));
    let resp = post_login(&app, form(&submit)).await;

    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()[header::CACHE_CONTROL], "no-store");
    let (error, state) = redirected_error(&resp);
    assert_eq!(error, "access_denied");
    assert_eq!(state.as_deref(), Some("state-123"));
}

#[r2e_core::test]
async fn login_requires_the_one_time_form_token() {
    let app = authorize_app().await;
    let challenge = challenge();
    let params = authorize_params(&challenge, &[]);
    let csrf = login_page(&app, &form(&params)).await;

    // A forged token — the cross-site case — is refused without redirecting.
    let mut forged = params.clone();
    forged.push(("csrf_token", "not-a-real-token"));
    forged.push(("username", "alice"));
    forged.push(("password", "password123"));
    let resp = post_login(&app, form(&forged)).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(resp.headers().get(header::LOCATION).is_none());
    assert_eq!(body_json(resp).await["error"], "invalid_request");

    // The real token works exactly once.
    let mut submit = params.clone();
    submit.push(("csrf_token", &csrf));
    submit.push(("username", "alice"));
    submit.push(("password", "password123"));
    let resp = post_login(&app, form(&submit)).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let resp = post_login(&app, form(&submit)).await;
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "a login form token must not be replayable"
    );
}

#[r2e_core::test]
async fn login_rejects_a_scope_outside_the_client_allowlist() {
    let app = authorize_app().await;
    let challenge = challenge();
    let params = authorize_params(&challenge, &[]);
    let csrf = login_page(&app, &form(&params)).await;

    let mut submit = authorize_params(&challenge, &[("scope", Some("mcp:admin"))]);
    submit.push(("csrf_token", &csrf));
    submit.push(("username", "alice"));
    submit.push(("password", "password123"));
    let resp = post_login(&app, form(&submit)).await;

    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(redirected_error(&resp).0, "invalid_scope");
}

/// A public client with no declared scopes may only receive an empty scope.
#[r2e_core::test]
async fn public_client_without_scopes_cannot_request_any() {
    let users = InMemoryUserStore::new().add_user(
        "alice",
        "password123",
        OidcUser {
            sub: "user-1".into(),
            ..Default::default()
        },
    );
    let app = r2e_core::AppBuilder::new()
        .plugin(
            OidcServer::new()
                .audience(AUTH_AUDIENCE)
                .with_user_store(users)
                .with_client_registry(
                    ClientRegistry::new().add_public_client("mcp-client", [AUTH_REDIRECT]),
                ),
        )
        .build_state()
        .await
        .build();

    let challenge = challenge();
    let query = form(&authorize_params(&challenge, &[("scope", Some("openid"))]));
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/oauth/authorize?{query}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(redirected_error(&resp).0, "invalid_scope");
}
