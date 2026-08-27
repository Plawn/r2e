//! End-to-end OAuth against a real Keycloak container (`DevKeycloak`).
//!
//! The whole production path runs — RFC 8414 discovery against the container,
//! RS256 JWKS validation, audience binding via the realm's audience mapper,
//! scope checks and `tools/list` filtering. Requires Docker, `#[ignore]`d by
//! default:
//!
//! ```text
//! cargo test -p r2e-mcp --test auth keycloak:: -- --ignored
//! ```

use r2e_core::http::{Router, StatusCode};
use r2e_devservices::DevKeycloak;
use r2e_mcp::auth::McpAuthConfig;
use r2e_mcp::McpServer;

use crate::fixtures::{
    self, get, initialize_auth, secured_plugin_app, tool_names, tools_call_auth, tools_list_auth,
};

/// Boot against the container's issuer with the REAL discovery + JWKS path —
/// no pinned validator. The resource is pinned to the URI the bundled realm's
/// audience mapper stamps into every token.
async fn keycloak_app(kc: &DevKeycloak) -> Router {
    secured_plugin_app(McpServer::new().with_auth(McpAuthConfig {
        issuer: kc.issuer(),
        resource: Some(fixtures::RESOURCE.to_string()),
        allow_insecure: Some(true),
        ..Default::default()
    }))
    .await
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn keycloak_tokens_drive_the_full_auth_path() {
    let kc = DevKeycloak::shared().await;
    let app = keycloak_app(&kc).await;

    // No token → challenged.
    let (status, headers, _) = get(&app, "/mcp", &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(headers.contains_key("www-authenticate"));

    // alice: realm roles admin+user, granted scope mcp:read only.
    let token = kc
        .password_token("alice", "alice-password", "test-cli", "mcp:read")
        .await;
    let session = initialize_auth(&app, "/mcp", &token).await;

    // tools/list is filtered by the caller's scopes and roles: without
    // mcp:write, `write_data` and `flexible` are hidden; `admin_only` shows
    // because alice's Keycloak realm role `admin` is merged by default.
    let list = tools_list_auth(&app, "/mcp", &session, &token).await;
    assert_eq!(
        tool_names(&list),
        ["admin_only", "ping", "read_data", "whoami"]
    );

    // The principal reaches the tool (sub = Keycloak's user id).
    let result = tools_call_auth(
        &app,
        "/mcp",
        &session,
        &token,
        "whoami",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(result["result"]["isError"], false);

    // Scope-gated tool passes with mcp:read…
    let result = tools_call_auth(
        &app,
        "/mcp",
        &session,
        &token,
        "read_data",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(result["result"]["isError"], false);

    // …and the missing mcp:write denies (JSON-RPC error, not HTTP).
    let result = tools_call_auth(
        &app,
        "/mcp",
        &session,
        &token,
        "write_data",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(result["error"]["data"], "forbidden", "{result}");
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn keycloak_roles_gate_tools() {
    let kc = DevKeycloak::shared().await;
    let app = keycloak_app(&kc).await;

    // bob has realm role `user` only: `admin_only` is hidden and denied.
    let token = kc
        .password_token("bob", "bob-password", "test-cli", "")
        .await;
    let session = initialize_auth(&app, "/mcp", &token).await;

    let list = tools_list_auth(&app, "/mcp", &session, &token).await;
    assert!(!tool_names(&list).contains(&"admin_only".to_string()));

    let result = tools_call_auth(
        &app,
        "/mcp",
        &session,
        &token,
        "admin_only",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(result["error"]["data"], "forbidden", "{result}");
}
