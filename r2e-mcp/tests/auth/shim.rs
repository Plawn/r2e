//! The static DCR shim: registration endpoint, redirect-URI allowlist,
//! mirrored authorization-server metadata.

use r2e_core::http::{Body, StatusCode};
use r2e_mcp::auth::{DiscoveryMode, McpAuthConfig};
use serde_json::{json, Value};

use crate::fixtures::{get, offline_auth, secured_app_with, send, ISSUER};

const REGISTER: &str = "/mcp/oauth/register";

fn shim_auth() -> McpAuthConfig {
    McpAuthConfig {
        public_client_id: Some("mcp-public".into()),
        ..offline_auth()
    }
}

async fn post_register(router: &r2e_core::http::Router, path: &str, body: &str) -> (StatusCode, Value) {
    let (status, _, raw) = send(
        router,
        "POST",
        path,
        &[("content-type", "application/json")],
        Body::from(body.to_string()),
    )
    .await;
    let parsed: Value =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("not JSON ({e}): {raw}"));
    (status, parsed)
}

// ── Registration ───────────────────────────────────────────────────────────

#[tokio::test]
async fn register_returns_the_static_public_client() {
    let router = secured_app_with(shim_auth()).await;
    let request = json!({
        "client_name": "Claude",
        "redirect_uris": [
            "https://claude.ai/api/mcp/auth_callback",   // default-allowlist exact
            "http://localhost:6274/oauth/callback",       // localhost any-port
            "https://evil.example/cb"                     // NOT on the allowlist
        ]
    });
    let (status, body) = post_register(&router, REGISTER, &request.to_string()).await;

    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["client_id"], "mcp-public");
    assert_eq!(body["token_endpoint_auth_method"], "none");
    assert_eq!(
        body["grant_types"],
        json!(["authorization_code", "refresh_token"])
    );
    assert_eq!(body["response_types"], json!(["code"]));
    assert_eq!(body["client_name"], "Claude");
    // The rogue redirect is silently dropped; nothing was actually
    // registered anywhere — the shim only echoes what the IdP client must
    // already contain.
    assert_eq!(
        body["redirect_uris"],
        json!([
            "https://claude.ai/api/mcp/auth_callback",
            "http://localhost:6274/oauth/callback"
        ])
    );
    assert!(body.get("client_secret").is_none());
    assert!(body.get("registration_access_token").is_none());
}

#[tokio::test]
async fn register_rejects_when_every_redirect_is_off_allowlist() {
    let router = secured_app_with(shim_auth()).await;
    let request = json!({ "redirect_uris": ["https://evil.example/cb"] });
    let (status, body) = post_register(&router, REGISTER, &request.to_string()).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_redirect_uri");
    assert_eq!(
        body["error_description"],
        "no requested redirect_uri is on the allowlist (`mcp.auth.redirect-uri-allowlist`)"
    );
}

#[tokio::test]
async fn register_rejects_non_json_bodies() {
    let router = secured_app_with(shim_auth()).await;
    let (status, body) = post_register(&router, REGISTER, "not json").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_client_metadata");
    assert_eq!(
        body["error_description"],
        "registration request is not a JSON object"
    );
}

#[tokio::test]
async fn register_caps_the_body_at_16_kib() {
    let router = secured_app_with(shim_auth()).await;
    let huge = format!(
        "{{\"client_name\":\"{}\"}}",
        "x".repeat(17 * 1024)
    );
    let (status, body) = post_register(&router, REGISTER, &huge).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body["error"], "invalid_client_metadata");
    assert_eq!(body["error_description"], "registration request body too large");
}

#[tokio::test]
async fn register_only_accepts_post() {
    let router = secured_app_with(shim_auth()).await;
    let (status, _, _) = get(&router, REGISTER, &[]).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn custom_allowlist_replaces_the_defaults() {
    let router = secured_app_with(McpAuthConfig {
        redirect_uri_allowlist: Some(vec!["https://my.app/callback".into()]),
        ..shim_auth()
    })
    .await;
    // The default claude.ai callback is now REJECTED…
    let request = json!({ "redirect_uris": ["https://claude.ai/api/mcp/auth_callback"] });
    let (status, body) = post_register(&router, REGISTER, &request.to_string()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    // …and only the custom entry passes.
    let request = json!({
        "redirect_uris": ["https://my.app/callback", "http://localhost:6274/cb"]
    });
    let (status, body) = post_register(&router, REGISTER, &request.to_string()).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["redirect_uris"], json!(["https://my.app/callback"]));
}

#[tokio::test]
async fn custom_registration_path_moves_the_endpoint() {
    let router = secured_app_with(McpAuthConfig {
        registration_path: Some("/register".into()),
        ..shim_auth()
    })
    .await;
    let request = json!({ "redirect_uris": ["http://localhost:6274/cb"] });
    let (status, body) = post_register(&router, "/mcp/register", &request.to_string()).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    // The metadata mirror advertises the moved endpoint.
    let (_, _, raw) = get(&router, "/.well-known/oauth-authorization-server", &[]).await;
    let doc: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        doc["registration_endpoint"],
        "http://localhost:3000/mcp/register"
    );
}

// ── Mirrored authorization-server metadata ─────────────────────────────────

const MIRROR_PATHS: [&str; 4] = [
    "/.well-known/oauth-authorization-server",
    "/.well-known/oauth-authorization-server/mcp",
    "/.well-known/openid-configuration",
    "/mcp/.well-known/openid-configuration",
];

#[tokio::test]
async fn mirror_keeps_the_idp_issuer_and_adds_shim_capabilities() {
    let router = secured_app_with(McpAuthConfig {
        scopes_supported: Some(vec!["mcp:read".into()]),
        ..shim_auth()
    })
    .await;
    for path in MIRROR_PATHS {
        let (status, _, raw) = get(&router, path, &[]).await;
        assert_eq!(status, StatusCode::OK, "{path}: {raw}");
        let doc: Value = serde_json::from_str(&raw).unwrap();
        // `issuer` stays the REAL IdP's — rewriting it would break the
        // client's `iss`/token_endpoint validation (RFC 8414 §3.3 tension,
        // deliberate).
        assert_eq!(doc["issuer"], ISSUER, "{path}");
        assert_eq!(
            doc["registration_endpoint"],
            "http://localhost:3000/mcp/oauth/register",
            "{path}"
        );
        let s256 = doc["code_challenge_methods_supported"]
            .as_array()
            .expect("code_challenge_methods_supported");
        assert!(s256.contains(&json!("S256")), "{path}: {doc}");
        let auth_methods = doc["token_endpoint_auth_methods_supported"]
            .as_array()
            .expect("token_endpoint_auth_methods_supported");
        assert!(auth_methods.contains(&json!("none")), "{path}: {doc}");
        let scopes = doc["scopes_supported"].as_array().expect("scopes_supported");
        assert!(scopes.contains(&json!("mcp:read")), "{path}: {doc}");
    }
}

#[tokio::test]
async fn mirror_answers_503_when_discovery_is_unavailable() {
    // Lazy discovery against a dead IdP: boot succeeds (no eager fetch, the
    // pinned validator needs no JWKS), but the mirror cannot serve metadata.
    let router = secured_app_with(McpAuthConfig {
        issuer: "http://127.0.0.1:1".into(),
        discovery: Some(DiscoveryMode::Lazy),
        public_client_id: Some("mcp-public".into()),
        ..offline_auth()
    })
    .await;
    let (status, _, raw) = get(&router, "/.well-known/oauth-authorization-server", &[]).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{raw}");
    let doc: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(doc["error"], "temporarily_unavailable");
    assert_eq!(
        doc["error_description"],
        "authorization server metadata is currently unavailable"
    );
}

// ── Authorize-redirect shim ────────────────────────────────────────────────

const AUTHORIZE: &str = "/mcp/oauth/authorize";

fn authorize_auth() -> McpAuthConfig {
    McpAuthConfig {
        authorization_endpoint: Some("http://idp.test/protocol/auth?p=1".into()),
        extra_authorize_params: Some(
            [
                ("audience".to_string(), "https://api.example".to_string()),
                ("access_type".to_string(), "offline".to_string()),
            ]
            .into_iter()
            .collect(),
        ),
        ..shim_auth()
    }
}

#[tokio::test]
async fn authorize_redirects_with_merged_params() {
    let router = secured_app_with(authorize_auth()).await;
    // `audience=evil` is client-sent and collides with server policy — the
    // configured value must win.
    let (status, headers, raw) = get(
        &router,
        "/mcp/oauth/authorize?client_id=mcp-public&state=xyz&audience=evil",
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::FOUND, "{raw}");
    let location = headers["location"].to_str().unwrap();
    // Endpoint's own query kept, client params appended (minus overridden
    // keys), then the extra params sorted by key.
    assert_eq!(
        location,
        "http://idp.test/protocol/auth?p=1&client_id=mcp-public&state=xyz\
         &access_type=offline&audience=https%3A%2F%2Fapi.example",
    );
    assert_eq!(headers["cache-control"], "no-store");
}

#[tokio::test]
async fn mirror_advertises_the_authorize_shim() {
    let router = secured_app_with(authorize_auth()).await;
    let (status, _, raw) = get(&router, "/.well-known/oauth-authorization-server", &[]).await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    let doc: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        doc["authorization_endpoint"],
        "http://localhost:3000/mcp/oauth/authorize"
    );
}

#[tokio::test]
async fn mirror_keeps_the_idp_authorize_endpoint_without_extra_params() {
    let router = secured_app_with(McpAuthConfig {
        authorization_endpoint: Some("http://idp.test/protocol/auth".into()),
        ..shim_auth()
    })
    .await;
    let (status, _, raw) = get(&router, "/.well-known/oauth-authorization-server", &[]).await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    let doc: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(doc["authorization_endpoint"], "http://idp.test/protocol/auth");
    // And the redirect endpoint is not mounted at all.
    let (status, _, _) = get(&router, AUTHORIZE, &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn authorize_answers_503_without_an_advertised_endpoint() {
    // `discovery: off` with no authorization-endpoint configured: the fixed
    // metadata has no authorization_endpoint to redirect to.
    let router = secured_app_with(McpAuthConfig {
        extra_authorize_params: Some(
            [("audience".to_string(), "https://api.example".to_string())]
                .into_iter()
                .collect(),
        ),
        ..shim_auth()
    })
    .await;
    let (status, _, raw) = get(&router, AUTHORIZE, &[]).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{raw}");
    let doc: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(doc["error"], "temporarily_unavailable");
    assert!(
        doc["error_description"]
            .as_str()
            .unwrap()
            .contains("mcp.auth.authorization-endpoint"),
        "{doc}"
    );
}
