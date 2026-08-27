//! Shared fixtures for the `auth` target: a scope/role-annotated
//! `#[mcp_routes]` service, offline boot helpers (pinned TestJwt validator +
//! `discovery: off` → zero network I/O), and bearer-aware HTTP helpers.

use std::sync::Arc;

use r2e_core::http::{Body, HeaderMap, Request, Router, StatusCode};
use r2e_core::prelude::*;
use r2e_core::AppBuilder;
use r2e_macros::mcp_routes;
use r2e_mcp::auth::{DiscoveryMode, McpAuthConfig, ScopePolicy};
use r2e_mcp::{AppBuilderMcpExt, McpServer, McpTokenValidator};
use r2e_security::AuthenticatedUser;
use r2e_test::TestJwt;
use serde_json::Value;
use tower::ServiceExt;

use crate::support;

/// The fixture issuer (plaintext → `allow-insecure: true` in the config).
pub const ISSUER: &str = "http://idp.test";
/// The canonical resource URI (also the pinned validator's audience).
pub const RESOURCE: &str = "http://localhost:3000/mcp";
/// The PRM URL derived from [`RESOURCE`] (asserted in challenges).
pub const PRM_URL: &str = "http://localhost:3000/.well-known/oauth-protected-resource/mcp";

/// A dead JWKS endpoint: any accidental fetch fails fast (connection
/// refused), keeping the offline invariant honest.
pub const DEAD_JWKS: &str = "http://127.0.0.1:1/jwks";

// ── The secured fixture service ────────────────────────────────────────────

#[controller]
pub struct SecuredTools {}

#[mcp_routes]
impl SecuredTools {
    /// No requirements: callable by anyone the layer lets through.
    #[tool]
    async fn ping(&self) -> &'static str {
        "pong"
    }

    /// Identity-reading tool (no scope requirements): proves the principal
    /// inserted by the auth layer reaches the tool.
    #[tool]
    async fn whoami(&self, #[inject(identity)] user: AuthenticatedUser) -> String {
        user.sub
    }

    /// Single required scope.
    #[tool(scopes = "mcp:read")]
    async fn read_data(&self) -> &'static str {
        "data"
    }

    /// ALL of several scopes (array form).
    #[tool(scopes = ["mcp:read", "mcp:write"])]
    async fn write_data(&self) -> &'static str {
        "written"
    }

    /// At least ONE of several scopes.
    #[tool(any_scopes = ["mcp:admin", "mcp:write"])]
    async fn flexible(&self) -> &'static str {
        "ok"
    }

    /// Role-gated (enforced by the shared guard machinery, recorded for
    /// `tools/list` filtering).
    #[tool]
    #[roles("admin")]
    async fn admin_only(&self, #[inject(identity)] user: AuthenticatedUser) -> String {
        format!("admin:{}", user.sub)
    }

    /// Open resource: visible/readable by anyone the layer lets through.
    #[resource(uri = "r2e://secured/info")]
    async fn info(&self) -> &'static str {
        "public info"
    }

    /// Scope-gated resource.
    #[resource(uri = "r2e://secured/report", scopes = "mcp:write")]
    async fn report(&self) -> &'static str {
        "confidential report"
    }

    /// Open prompt.
    #[prompt]
    async fn howto(&self) -> &'static str {
        "Call `ping` first."
    }

    /// Scope-gated prompt.
    #[prompt(scopes = "mcp:write")]
    async fn write_recipe(&self) -> &'static str {
        "Call `write_data` with the payload."
    }
}

// ── Boot helpers ───────────────────────────────────────────────────────────

/// The TestJwt whose tokens the pinned validator accepts: issuer [`ISSUER`],
/// audience [`RESOURCE`].
pub fn test_jwt() -> TestJwt {
    TestJwt::with_config(b"auth-target-secret", ISSUER, RESOURCE)
}

/// The offline `mcp.auth` section: explicit resource, `discovery: off`, a
/// dead JWKS URL and `allow-insecure` for the plaintext fixture issuer.
pub fn offline_auth() -> McpAuthConfig {
    McpAuthConfig {
        issuer: ISSUER.to_string(),
        resource: Some(RESOURCE.to_string()),
        discovery: Some(DiscoveryMode::Off),
        jwks_url: Some(DEAD_JWKS.to_string()),
        allow_insecure: Some(true),
        ..Default::default()
    }
}

/// A validator pinned to `jwt`'s HS256 secret (no JWKS, no network).
pub fn pinned(jwt: &TestJwt) -> McpTokenValidator {
    McpTokenValidator::jwt(Arc::new(jwt.claims_validator()), ScopePolicy::default())
}

/// Boot the secured fixture app: [`offline_auth`] + pinned validator.
pub async fn secured_app() -> Router {
    secured_app_with(offline_auth()).await
}

/// Boot with a custom auth section (validator still pinned to
/// [`test_jwt`]).
pub async fn secured_app_with(auth: McpAuthConfig) -> Router {
    secured_plugin_app(
        McpServer::new()
            .with_auth(auth)
            .with_token_validator(pinned(&test_jwt())),
    )
    .await
}

/// Boot with a fully custom plugin (e.g. no pinned validator, or no auth).
pub async fn secured_plugin_app(plugin: McpServer) -> Router {
    AppBuilder::new()
        .plugin(plugin)
        .build_state()
        .await
        .register_mcp_service::<SecuredTools>()
        .build()
}

// ── HTTP helpers ───────────────────────────────────────────────────────────

/// Plain GET (well-known documents, shim metadata).
pub async fn get(router: &Router, path: &str, headers: &[(&str, &str)]) -> (StatusCode, HeaderMap, String) {
    send(router, "GET", path, headers, Body::empty()).await
}

/// Send an arbitrary request and collect the response.
pub async fn send(
    router: &Router,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Body,
) -> (StatusCode, HeaderMap, String) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "localhost");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let (parts, body) = response.into_parts();
    use http_body_util::BodyExt;
    let bytes = body.collect().await.unwrap().to_bytes();
    (
        parts.status,
        parts.headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

/// POST a JSON-RPC body with a bearer token.
pub async fn post_auth(
    router: &Router,
    path: &str,
    session: Option<&str>,
    token: &str,
    body: &Value,
) -> support::McpResponse {
    let bearer = format!("Bearer {token}");
    support::post_with_headers(router, path, session, &[("authorization", bearer.as_str())], body)
        .await
}

/// Authenticated session handshake (initialize → session id →
/// `notifications/initialized`).
pub async fn initialize_auth(router: &Router, path: &str, token: &str) -> String {
    let response = post_auth(router, path, None, token, &support::initialize_body()).await;
    assert_eq!(
        response.status,
        StatusCode::OK,
        "initialize failed: {}",
        response.raw_body
    );
    let session = response.session_id.clone().expect("no Mcp-Session-Id");
    let notified = post_auth(
        router,
        path,
        Some(&session),
        token,
        &serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await;
    assert_eq!(notified.status, StatusCode::ACCEPTED);
    session
}

/// Authenticated `tools/list`; returns the `result` object.
pub async fn tools_list_auth(router: &Router, path: &str, session: &str, token: &str) -> Value {
    let response = post_auth(
        router,
        path,
        Some(session),
        token,
        &serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);
    response.result().clone()
}

/// Authenticated `tools/call`; returns the full JSON-RPC message.
pub async fn tools_call_auth(
    router: &Router,
    path: &str,
    session: &str,
    token: &str,
    name: &str,
    arguments: Value,
) -> Value {
    let response = post_auth(
        router,
        path,
        Some(session),
        token,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        }),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);
    response.message().clone()
}

/// Authenticated JSON-RPC request with arbitrary method/params; returns the
/// full JSON-RPC message.
pub async fn rpc_auth(
    router: &Router,
    path: &str,
    session: &str,
    token: &str,
    method: &str,
    params: Value,
) -> Value {
    let response = post_auth(
        router,
        path,
        Some(session),
        token,
        &serde_json::json!({ "jsonrpc": "2.0", "id": 9, "method": method, "params": params }),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);
    response.message().clone()
}

/// The names in a `tools/list` result, sorted.
pub fn tool_names(list_result: &Value) -> Vec<String> {
    let mut names: Vec<String> = list_result["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    names.sort();
    names
}
