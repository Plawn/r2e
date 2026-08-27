//! The auth layer's HTTP-level behavior: challenges, denials, pass-throughs.

use r2e_core::http::{HeaderMap, StatusCode};
use r2e_mcp::auth::McpAuthConfig;
use r2e_mcp::McpServer;
use serde_json::Value;

use crate::fixtures::{
    initialize_auth, offline_auth, pinned, post_auth, secured_app, secured_app_with,
    secured_plugin_app, test_jwt, PRM_URL,
};
use crate::support;

fn www_authenticate(headers: &HeaderMap) -> &str {
    headers
        .get("www-authenticate")
        .expect("missing WWW-Authenticate")
        .to_str()
        .unwrap()
}

fn error_body(raw: &str) -> (String, String) {
    let body: Value = serde_json::from_str(raw).unwrap_or_else(|e| panic!("not JSON ({e}): {raw}"));
    (
        body["error"].as_str().unwrap_or_default().to_string(),
        body["error_description"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

#[tokio::test]
async fn missing_token_gets_401_with_resource_metadata_challenge() {
    let router = secured_app().await;
    let response = support::post(&router, "/mcp", None, &support::initialize_body()).await;

    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    let (code, description) = error_body(&response.raw_body);
    assert_eq!(code, "unauthorized");
    assert_eq!(description, "missing bearer token");
    // The exact RFC 9728 challenge — what turns the 401 into a working
    // OAuth flow. No `error` param on a missing (vs invalid) token.
    // (`raw_headers` is not exposed by support::post; re-issue via oneshot.)
    let (status, headers, _) = crate::fixtures::send(
        &router,
        "POST",
        "/mcp",
        &[
            ("content-type", "application/json"),
            ("accept", "application/json, text/event-stream"),
        ],
        r2e_core::http::Body::from(support::initialize_body().to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        www_authenticate(&headers),
        format!("Bearer resource_metadata=\"{PRM_URL}\"")
    );
}

#[tokio::test]
async fn malformed_token_gets_401_invalid_token() {
    let router = secured_app().await;
    let token = r2e_test::TestJwt::malformed_token();
    let (status, headers, body) = crate::fixtures::send(
        &router,
        "POST",
        "/mcp",
        &[
            ("content-type", "application/json"),
            ("accept", "application/json, text/event-stream"),
            ("authorization", &format!("Bearer {token}")),
        ],
        r2e_core::http::Body::from(support::initialize_body().to_string()),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (code, description) = error_body(&body);
    assert_eq!(code, "invalid_token");
    // Static allow-listed reason — never token contents or crypto detail.
    assert_eq!(description, "token validation failed");
    assert_eq!(
        www_authenticate(&headers),
        format!(
            "Bearer resource_metadata=\"{PRM_URL}\", error=\"invalid_token\", \
             error_description=\"token validation failed\""
        )
    );
}

#[tokio::test]
async fn expired_token_gets_the_expired_reason() {
    let router = secured_app().await;
    let token = test_jwt().token_builder("alice").expired().build();
    let response = post_auth(&router, "/mcp", None, &token, &support::initialize_body()).await;

    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    let (code, description) = error_body(&response.raw_body);
    assert_eq!((code.as_str(), description.as_str()), ("invalid_token", "token expired"));
}

#[tokio::test]
async fn wrong_audience_and_wrong_issuer_are_rejected() {
    let router = secured_app().await;
    for token in [
        test_jwt().wrong_audience_token("alice"),
        test_jwt().wrong_issuer_token("alice"),
    ] {
        let response = post_auth(&router, "/mcp", None, &token, &support::initialize_body()).await;
        assert_eq!(response.status, StatusCode::UNAUTHORIZED, "{}", response.raw_body);
        assert_eq!(error_body(&response.raw_body).0, "invalid_token");
    }
}

#[tokio::test]
async fn valid_token_reaches_the_mcp_service() {
    let router = secured_app().await;
    let token = test_jwt().token("alice", &[]);
    // The full session dance succeeds — the layer inserted the principal and
    // handed the request to rmcp.
    let session = initialize_auth(&router, "/mcp", &token).await;
    assert!(!session.is_empty());
}

#[tokio::test]
async fn missing_required_scope_gets_403_with_missing_scopes() {
    let router = secured_app_with(McpAuthConfig {
        required_scopes: Some(vec!["mcp:read".into(), "mcp:write".into()]),
        ..offline_auth()
    })
    .await;
    let token = test_jwt().token_builder("bob").scopes(&["mcp:read"]).build();
    let (status, headers, body) = crate::fixtures::send(
        &router,
        "POST",
        "/mcp",
        &[
            ("content-type", "application/json"),
            ("accept", "application/json, text/event-stream"),
            ("authorization", &format!("Bearer {token}")),
        ],
        r2e_core::http::Body::from(support::initialize_body().to_string()),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    let (code, description) = error_body(&body);
    assert_eq!(code, "insufficient_scope");
    assert_eq!(description, "token lacks a required scope");
    // Only the MISSING scopes are advertised, space-joined.
    let challenge = www_authenticate(&headers);
    assert!(challenge.contains("error=\"insufficient_scope\""), "{challenge}");
    assert!(challenge.contains("scope=\"mcp:write\""), "{challenge}");
    assert!(!challenge.contains("mcp:read"), "{challenge}");
}

#[tokio::test]
async fn required_scopes_present_passes() {
    let router = secured_app_with(McpAuthConfig {
        required_scopes: Some(vec!["mcp:read".into()]),
        ..offline_auth()
    })
    .await;
    let token = test_jwt().token_builder("bob").scopes(&["mcp:read"]).build();
    initialize_auth(&router, "/mcp", &token).await;
}

#[tokio::test]
async fn cors_preflight_bypasses_auth() {
    let router = secured_app().await;
    // A real preflight (OPTIONS + Origin + requested method): answered by
    // the CORS layer, never challenged — it cannot carry Authorization.
    let (status, headers, _) = crate::fixtures::send(
        &router,
        "OPTIONS",
        "/mcp",
        &[
            ("origin", "https://claude.ai"),
            ("access-control-request-method", "POST"),
            ("access-control-request-headers", "authorization, content-type"),
        ],
        r2e_core::http::Body::empty(),
    )
    .await;
    assert_ne!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        headers
            .get("access-control-allow-origin")
            .map(|v| v.to_str().unwrap()),
        Some("https://claude.ai")
    );
    // The preflight must clear the Authorization header for the real call.
    let allow_headers = headers
        .get("access-control-allow-headers")
        .map(|v| v.to_str().unwrap().to_ascii_lowercase())
        .unwrap_or_default();
    assert!(allow_headers.contains("authorization"), "{allow_headers}");

    // Expose-headers land on ACTUAL responses (not preflights): a browser
    // client must be able to read the challenge and the session id off the
    // 401 that starts the OAuth flow.
    let (status, headers, _) = crate::fixtures::send(
        &router,
        "POST",
        "/mcp",
        &[
            ("origin", "https://claude.ai"),
            ("content-type", "application/json"),
            ("accept", "application/json, text/event-stream"),
        ],
        r2e_core::http::Body::from(support::initialize_body().to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let exposed = headers
        .get("access-control-expose-headers")
        .map(|v| v.to_str().unwrap().to_ascii_lowercase())
        .unwrap_or_default();
    assert!(exposed.contains("www-authenticate"), "{exposed}");
    assert!(exposed.contains("mcp-session-id"), "{exposed}");
}

#[tokio::test]
async fn disallowed_origin_gets_403_invalid_origin() {
    let router = secured_plugin_app(
        McpServer::new()
            .with_auth(offline_auth())
            .with_token_validator(pinned(&test_jwt()))
            .with_allowed_origins(["http://localhost:5173"]),
    )
    .await;
    let token = test_jwt().token("alice", &[]);
    let response = support::post_with_headers(
        &router,
        "/mcp",
        None,
        &[
            ("authorization", &format!("Bearer {token}")),
            ("origin", "http://evil.test"),
        ],
        &support::initialize_body(),
    )
    .await;

    assert_eq!(response.status, StatusCode::FORBIDDEN);
    let (code, description) = error_body(&response.raw_body);
    assert_eq!((code.as_str(), description.as_str()), ("invalid_origin", "origin not allowed"));
}

#[tokio::test]
async fn allowed_and_absent_origins_pass() {
    let router = secured_plugin_app(
        McpServer::new()
            .with_auth(offline_auth())
            .with_token_validator(pinned(&test_jwt()))
            .with_allowed_origins(["http://localhost:5173"]),
    )
    .await;
    let token = test_jwt().token("alice", &[]);
    // Origin on the allowlist.
    let response = support::post_with_headers(
        &router,
        "/mcp",
        None,
        &[
            ("authorization", &format!("Bearer {token}")),
            ("origin", "http://localhost:5173"),
        ],
        &support::initialize_body(),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);
    // No Origin header (non-browser client): the check does not apply.
    let response = post_auth(&router, "/mcp", None, &token, &support::initialize_body()).await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);
}

#[tokio::test]
async fn unreachable_jwks_gets_503_without_challenge() {
    // NO pinned validator: the plugin installs the lazy JWT backend, whose
    // first validation builds a JwksCache against the dead JWKS URL —
    // connection refused ⇒ Upstream ⇒ 503. No WWW-Authenticate: an IdP
    // outage must not send clients into a re-auth loop.
    let router = secured_plugin_app(McpServer::new().with_auth(offline_auth())).await;
    let token = test_jwt().token("alice", &[]);
    let (status, headers, body) = crate::fixtures::send(
        &router,
        "POST",
        "/mcp",
        &[
            ("content-type", "application/json"),
            ("accept", "application/json, text/event-stream"),
            ("authorization", &format!("Bearer {token}")),
        ],
        r2e_core::http::Body::from(support::initialize_body().to_string()),
    )
    .await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(headers.get("www-authenticate").is_none());
    let (code, description) = error_body(&body);
    assert_eq!(
        (code.as_str(), description.as_str()),
        ("unauthorized", "authorization server unavailable")
    );
}
