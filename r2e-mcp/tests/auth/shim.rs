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
