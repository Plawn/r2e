//! Protected-resource metadata (RFC 9728): both routes, exact body shape,
//! CORS/caching headers, the shim's `authorization_servers` flip.

use r2e_core::http::StatusCode;
use r2e_mcp::auth::McpAuthConfig;
use serde_json::Value;

use crate::fixtures::{get, offline_auth, secured_app, secured_app_with, ISSUER, RESOURCE};

const PRM_ROOT: &str = "/.well-known/oauth-protected-resource";
const PRM_SUFFIXED: &str = "/.well-known/oauth-protected-resource/mcp";

async fn prm(router: &r2e_core::http::Router, path: &str) -> Value {
    let (status, _, body) = get(router, path, &[]).await;
    assert_eq!(status, StatusCode::OK, "{path}: {body}");
    serde_json::from_str(&body).unwrap()
}

#[tokio::test]
async fn both_prm_routes_serve_the_same_minimal_document() {
    let router = secured_app().await;
    for path in [PRM_ROOT, PRM_SUFFIXED] {
        let doc = prm(&router, path).await;
        assert_eq!(doc["resource"], RESOURCE, "{path}");
        assert_eq!(doc["authorization_servers"], serde_json::json!([ISSUER]));
        assert_eq!(doc["bearer_methods_supported"], serde_json::json!(["header"]));
        // Optional keys are ABSENT when unset — clients treat null and
        // missing differently.
        assert!(doc.get("scopes_supported").is_none(), "{doc}");
        assert!(doc.get("resource_name").is_none(), "{doc}");
    }
}

#[tokio::test]
async fn prm_is_unauthenticated_with_cors_and_cache_headers() {
    let router = secured_app().await;
    // No bearer token on the GET — this is the document a client fetches
    // BEFORE it has a token.
    let (status, headers, _) = get(&router, PRM_SUFFIXED, &[]).await;
    assert_eq!(status, StatusCode::OK);
    let header = |name: &str| {
        headers
            .get(name)
            .unwrap_or_else(|| panic!("missing {name}"))
            .to_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(header("cache-control"), "public, max-age=300");
    assert_eq!(header("content-type"), "application/json");
    assert_eq!(header("access-control-allow-origin"), "*");
    assert_eq!(header("access-control-allow-methods"), "GET, POST, OPTIONS");
    assert_eq!(
        header("access-control-allow-headers"),
        "authorization, content-type, mcp-protocol-version"
    );
    assert_eq!(
        header("access-control-expose-headers"),
        "www-authenticate, mcp-session-id"
    );
}

#[tokio::test]
async fn prm_answers_options_preflight() {
    let router = secured_app().await;
    let (status, headers, _) = crate::fixtures::send(
        &router,
        "OPTIONS",
        PRM_SUFFIXED,
        &[("origin", "https://claude.ai")],
        r2e_core::http::Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        headers.get("access-control-allow-origin").unwrap(),
        "*"
    );
}

#[tokio::test]
async fn scopes_supported_falls_back_to_required_scopes() {
    let router = secured_app_with(McpAuthConfig {
        required_scopes: Some(vec!["mcp:read".into()]),
        ..offline_auth()
    })
    .await;
    let doc = prm(&router, PRM_SUFFIXED).await;
    assert_eq!(doc["scopes_supported"], serde_json::json!(["mcp:read"]));
}

#[tokio::test]
async fn explicit_scopes_supported_and_resource_name_are_echoed() {
    let router = secured_app_with(McpAuthConfig {
        scopes_supported: Some(vec!["mcp:read".into(), "mcp:write".into()]),
        resource_name: Some("Fixture MCP".into()),
        ..offline_auth()
    })
    .await;
    let doc = prm(&router, PRM_SUFFIXED).await;
    assert_eq!(
        doc["scopes_supported"],
        serde_json::json!(["mcp:read", "mcp:write"])
    );
    assert_eq!(doc["resource_name"], "Fixture MCP");
}

#[tokio::test]
async fn shim_flips_authorization_servers_to_the_resource_origin() {
    // `public-client-id` set ⇒ shim on by default ⇒ clients are pointed at
    // OUR origin (which mirrors the IdP metadata + adds registration).
    let router = secured_app_with(McpAuthConfig {
        public_client_id: Some("mcp-public".into()),
        ..offline_auth()
    })
    .await;
    let doc = prm(&router, PRM_SUFFIXED).await;
    assert_eq!(
        doc["authorization_servers"],
        serde_json::json!(["http://localhost:3000"])
    );
}

#[tokio::test]
async fn shim_false_keeps_the_real_issuer_even_with_a_client_id() {
    let router = secured_app_with(McpAuthConfig {
        public_client_id: Some("mcp-public".into()),
        shim: Some(false),
        ..offline_auth()
    })
    .await;
    let doc = prm(&router, PRM_SUFFIXED).await;
    assert_eq!(doc["authorization_servers"], serde_json::json!([ISSUER]));
}
