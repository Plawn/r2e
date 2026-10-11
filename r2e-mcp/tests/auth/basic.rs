//! HTTP Basic (`mcp.auth.basic`): alone and next to OAuth.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use r2e_core::http::{Body, HeaderMap, Router, StatusCode};
use r2e_mcp::auth::{McpAuthConfig, McpBasicAuthConfig};
use r2e_mcp::McpServer;
use serde_json::{json, Value};

use crate::fixtures::{
    initialize_auth, offline_auth, pinned, secured_app_with, secured_plugin_app, send, test_jwt,
    PRM_URL,
};
use crate::support;

const USER: &str = "agent";
const PASSWORD: &str = "s3cret-pa55";

fn basic_section() -> McpBasicAuthConfig {
    McpBasicAuthConfig {
        username: USER.into(),
        password: PASSWORD.into(),
        realm: None,
        roles: Some(vec!["admin".into()]),
        scopes: Some(vec!["mcp:read".into()]),
    }
}

/// `Authorization` value for a user-id/password pair.
fn basic(user: &str, password: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{user}:{password}")))
}

/// Basic only: no issuer, no pinned validator, nothing OAuth.
async fn basic_only_app(auth: McpAuthConfig) -> Router {
    secured_plugin_app(McpServer::new().with_auth(auth)).await
}

fn basic_only_config() -> McpAuthConfig {
    McpAuthConfig {
        basic: Some(basic_section()),
        ..Default::default()
    }
}

/// Every `WWW-Authenticate` value, in header order.
fn challenges(headers: &HeaderMap) -> Vec<String> {
    headers
        .get_all("www-authenticate")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect()
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

/// POST `initialize` with the given `Authorization` header (or none);
/// returns status, headers and body.
async fn initialize_with(
    router: &Router,
    authorization: Option<&str>,
) -> (StatusCode, HeaderMap, String) {
    let mut headers = vec![
        ("content-type", "application/json"),
        ("accept", "application/json, text/event-stream"),
    ];
    if let Some(value) = authorization {
        headers.push(("authorization", value));
    }
    send(
        router,
        "POST",
        "/mcp",
        &headers,
        Body::from(support::initialize_body().to_string()),
    )
    .await
}

/// Full handshake with Basic credentials; returns the session id.
async fn initialize_basic(router: &Router, authorization: &str) -> String {
    support::initialize_with_headers(router, "/mcp", &[("authorization", authorization)]).await
}

async fn call(router: &Router, session: &str, authorization: &str, tool: &str) -> Value {
    support::tools_call_with_headers(
        router,
        "/mcp",
        session,
        &[("authorization", authorization)],
        tool,
        json!({}),
    )
    .await
}

// ── Basic only ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn basic_only_challenges_with_basic_and_mounts_no_oauth_surface() {
    let router = basic_only_app(basic_only_config()).await;

    let (status, headers, body) = initialize_with(&router, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        challenges(&headers),
        vec!["Basic realm=\"mcp\", charset=\"UTF-8\"".to_string()],
        "no Bearer challenge without an issuer"
    );
    assert_eq!(
        error_body(&body),
        (
            "unauthorized".to_string(),
            "missing credentials".to_string()
        )
    );

    // No protected-resource metadata: there is no authorization server to
    // point at.
    let (status, _, _) = send(
        &router,
        "GET",
        "/.well-known/oauth-protected-resource/mcp",
        &[],
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn valid_credentials_authenticate_as_the_configured_principal() {
    let router = basic_only_app(basic_only_config()).await;
    let auth = basic(USER, PASSWORD);
    let session = initialize_basic(&router, &auth).await;

    // `sub` is the user-id.
    let whoami = call(&router, &session, &auth, "whoami").await;
    assert_eq!(whoami["result"]["content"][0]["text"], USER);
    // Configured roles feed `#[roles]`…
    let admin = call(&router, &session, &auth, "admin_only").await;
    assert_eq!(
        admin["result"]["content"][0]["text"],
        format!("admin:{USER}")
    );
    // …and configured scopes feed `#[tool(scopes)]`, including the
    // visibility filter: `write_data` needs `mcp:write`, which this
    // principal lacks.
    let read = call(&router, &session, &auth, "read_data").await;
    assert_eq!(read["result"]["content"][0]["text"], "data");
    let list = support::post_with_headers(
        &router,
        "/mcp",
        Some(&session),
        &[("authorization", auth.as_str())],
        &json!({ "jsonrpc": "2.0", "id": 9, "method": "tools/list" }),
    )
    .await;
    assert_eq!(list.status, StatusCode::OK, "{}", list.raw_body);
    let names = support::tool_names(list.result());
    assert!(names.contains(&"read_data".to_string()), "{names:?}");
    assert!(!names.contains(&"write_data".to_string()), "{names:?}");
}

#[tokio::test]
async fn wrong_or_malformed_credentials_are_401_invalid_token() {
    let router = basic_only_app(basic_only_config()).await;
    let cases: [(&str, String, &str); 5] = [
        ("wrong password", basic(USER, "nope"), "invalid credentials"),
        (
            "wrong user",
            basic("someone", PASSWORD),
            "invalid credentials",
        ),
        ("empty pair", basic("", ""), "invalid credentials"),
        (
            "no colon",
            format!("Basic {}", STANDARD.encode(USER)),
            "malformed basic credentials",
        ),
        (
            "not base64",
            "Basic %%%not-base64%%%".to_string(),
            "malformed basic credentials",
        ),
    ];
    for (case, header, expected) in cases {
        let (status, headers, body) = initialize_with(&router, Some(&header)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{case}");
        assert_eq!(
            error_body(&body),
            ("invalid_token".to_string(), expected.to_string()),
            "{case}"
        );
        assert_eq!(
            challenges(&headers),
            vec!["Basic realm=\"mcp\", charset=\"UTF-8\"".to_string()],
            "{case}"
        );
    }
}

#[tokio::test]
async fn bearer_on_a_basic_only_endpoint_is_an_unsupported_scheme() {
    let router = basic_only_app(basic_only_config()).await;
    let token = format!("Bearer {}", test_jwt().token(USER, &[]));
    let (status, _, body) = initialize_with(&router, Some(&token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        error_body(&body),
        (
            "invalid_token".to_string(),
            "unsupported authorization scheme".to_string()
        )
    );
}

#[tokio::test]
async fn scheme_match_is_case_insensitive_and_realm_is_configurable() {
    let router = basic_only_app(McpAuthConfig {
        basic: Some(McpBasicAuthConfig {
            realm: Some("Agents \"prod\"".into()),
            ..basic_section()
        }),
        ..Default::default()
    })
    .await;
    let (status, headers, _) = initialize_with(&router, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        challenges(&headers),
        vec!["Basic realm=\"Agents \\\"prod\\\"\", charset=\"UTF-8\"".to_string()]
    );

    let lower = basic(USER, PASSWORD).replacen("Basic", "basic", 1);
    let (status, _, _) = initialize_with(&router, Some(&lower)).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn required_scopes_apply_to_basic_principals() {
    let router = basic_only_app(McpAuthConfig {
        required_scopes: Some(vec!["mcp:write".into()]),
        ..basic_only_config()
    })
    .await;
    let auth = basic(USER, PASSWORD);
    let (status, headers, body) = initialize_with(&router, Some(&auth)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error_body(&body).0, "insufficient_scope");
    // A 403 is not an invitation to retry with other credentials: no Basic
    // challenge (and no Bearer one — there is no OAuth surface).
    assert!(challenges(&headers).is_empty(), "{headers:?}");
}

#[tokio::test]
#[should_panic(expected = "mcp.auth.basic.username `a:b` must not contain ':'")]
async fn a_colon_in_the_username_is_a_boot_error() {
    let _ = basic_only_app(McpAuthConfig {
        basic: Some(McpBasicAuthConfig {
            username: "a:b".into(),
            ..basic_section()
        }),
        ..Default::default()
    })
    .await;
}

#[tokio::test]
#[should_panic(expected = "mcp.auth.basic.password must not be empty")]
async fn an_empty_password_is_a_boot_error() {
    let _ = basic_only_app(McpAuthConfig {
        basic: Some(McpBasicAuthConfig {
            password: String::new(),
            ..basic_section()
        }),
        ..Default::default()
    })
    .await;
}

#[tokio::test]
#[should_panic(expected = "mcp.auth.issuer must not be empty")]
async fn neither_issuer_nor_basic_is_a_boot_error() {
    let _ = basic_only_app(McpAuthConfig::default()).await;
}

// ── Basic next to OAuth ────────────────────────────────────────────────────

fn mixed_config() -> McpAuthConfig {
    McpAuthConfig {
        basic: Some(basic_section()),
        ..offline_auth()
    }
}

#[tokio::test]
async fn both_schemes_are_accepted_and_both_challenges_advertised() {
    let router = secured_app_with(mixed_config()).await;

    // Missing credentials: Bearer challenge first (OAuth-aware MCP clients
    // read the first one), then Basic.
    let (status, headers, _) = initialize_with(&router, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        challenges(&headers),
        vec![
            format!("Bearer resource_metadata=\"{PRM_URL}\""),
            "Basic realm=\"mcp\", charset=\"UTF-8\"".to_string(),
        ]
    );

    // Bearer path untouched.
    let token = test_jwt().token("bob", &[]);
    let bearer_session = initialize_auth(&router, "/mcp", &token).await;
    let whoami = call(
        &router,
        &bearer_session,
        &format!("Bearer {token}"),
        "whoami",
    )
    .await;
    assert_eq!(whoami["result"]["content"][0]["text"], "bob");

    // Basic path alongside.
    let auth = basic(USER, PASSWORD);
    let basic_session = initialize_basic(&router, &auth).await;
    let whoami = call(&router, &basic_session, &auth, "whoami").await;
    assert_eq!(whoami["result"]["content"][0]["text"], USER);

    // A bad Basic pair on a mixed endpoint is still `invalid_token`, with
    // both challenges.
    let (status, headers, body) = initialize_with(&router, Some(&basic(USER, "nope"))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(error_body(&body).1, "invalid credentials");
    assert_eq!(challenges(&headers).len(), 2);
}

#[tokio::test]
async fn sessions_are_bound_to_the_basic_principal() {
    let router = secured_plugin_app(
        McpServer::new()
            .with_auth(mixed_config())
            .with_token_validator(pinned(&test_jwt())),
    )
    .await;
    let auth = basic(USER, PASSWORD);
    let session = initialize_basic(&router, &auth).await;

    // Another principal (a bearer user) presenting the Basic session's id
    // gets rmcp's own "session not found" — no oracle, no takeover.
    let token = test_jwt().token("bob", &[]);
    let response = support::post_with_headers(
        &router,
        "/mcp",
        Some(&session),
        &[("authorization", &format!("Bearer {token}"))],
        &json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/list" }),
    )
    .await;
    assert_eq!(
        response.status,
        StatusCode::NOT_FOUND,
        "{}",
        response.raw_body
    );
}
