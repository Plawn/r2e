//! Per-tool authorization: `check_access`/`requirements_visible` unit matrix,
//! then the end-to-end story — `tools/list` filtering and denial shapes over
//! the wire.

use r2e_core::http::Extensions;
use r2e_mcp::auth::tools::{check_access, requirements_visible};
use r2e_mcp::auth::McpAuthConfig;
use r2e_mcp::{McpError, McpPrincipal, ToolRequirements};
use serde_json::json;

use crate::fixtures::{
    initialize_auth, offline_auth, pinned, rpc_auth, secured_app, secured_app_with, test_jwt,
    tool_names, tools_call_auth, tools_list_auth,
};

// ── Unit matrix ────────────────────────────────────────────────────────────

/// A real principal (AuthenticatedUser is not literal-constructible): mint a
/// token and run it through the pinned validator.
async fn principal(scopes: &[&str], roles: &[&str]) -> McpPrincipal {
    let jwt = test_jwt();
    let token = jwt.token_builder("alice").scopes(scopes).roles(roles).build();
    pinned(&jwt).validate(&token).await.expect("valid fixture token")
}

fn ext_with<T: Clone + Send + Sync + 'static>(value: T) -> Extensions {
    let mut ext = Extensions::new();
    ext.insert(value);
    ext
}

const READ: ToolRequirements = ToolRequirements {
    scopes: &["mcp:read"],
    ..ToolRequirements::NONE
};

#[tokio::test]
async fn check_access_unit_matrix() {
    let ext = ext_with(principal(&["mcp:read"], &[]).await);

    // No scope requirements → allowed with or without any extensions.
    assert!(check_access(None, "tool", "t", &ToolRequirements::NONE).is_ok());
    // Requirements but no principal (auth disabled or layer bypassed) → fail closed.
    match check_access(Some(&Extensions::new()), "tool", "t", &READ) {
        Err(McpError::Unauthorized(msg)) => {
            assert_eq!(msg, "tool `t` requires an authenticated caller")
        }
        other => panic!("expected Unauthorized, got {other:?}"),
    }
    // Held scope → ok; the member kind only flavours the message.
    assert!(check_access(Some(&ext), "tool", "t", &READ).is_ok());
    // Missing scopes are named, joined, with re-authorize guidance.
    let all = ToolRequirements {
        scopes: &["mcp:read", "mcp:write", "mcp:admin"],
        ..ToolRequirements::NONE
    };
    match check_access(Some(&ext), "resource", "r", &all) {
        Err(McpError::Forbidden(msg)) => assert_eq!(
            msg,
            "resource `r` requires scope(s) `mcp:write, mcp:admin` that the token does not \
             carry; re-authorize requesting them"
        ),
        other => panic!("expected Forbidden, got {other:?}"),
    }
    // any_scopes: one match suffices; none → denial names the alternatives.
    let any = ToolRequirements {
        any_scopes: &["mcp:admin", "mcp:read"],
        ..ToolRequirements::NONE
    };
    assert!(check_access(Some(&ext), "tool", "t", &any).is_ok());
    let any_miss = ToolRequirements {
        any_scopes: &["mcp:admin", "mcp:write"],
        ..ToolRequirements::NONE
    };
    match check_access(Some(&ext), "prompt", "p", &any_miss) {
        Err(McpError::Forbidden(msg)) => assert_eq!(
            msg,
            "prompt `p` requires at least one of the scopes `mcp:admin, mcp:write`; \
             re-authorize requesting one"
        ),
        other => panic!("expected Forbidden, got {other:?}"),
    }
    // Roles are NOT check_access's job (the shared guard enforces them):
    // a role-only requirement passes here even without a principal.
    let admin_req = ToolRequirements {
        roles: &["admin"],
        ..ToolRequirements::NONE
    };
    assert!(check_access(Some(&Extensions::new()), "tool", "t", &admin_req).is_ok());
}

#[tokio::test]
async fn requirements_visible_covers_scopes_and_roles() {
    let admin_req = ToolRequirements {
        roles: &["admin"],
        ..ToolRequirements::NONE
    };
    // Roles ARE part of visibility (unlike check_access, which leaves role
    // enforcement to the guard).
    let admin = principal(&[], &["admin"]).await;
    let plain = principal(&["mcp:read"], &[]).await;
    assert!(requirements_visible(Some(&admin), &admin_req));
    assert!(!requirements_visible(Some(&plain), &admin_req));
    assert!(requirements_visible(Some(&plain), &READ));
    assert!(!requirements_visible(Some(&admin), &READ));
    // Unrestricted → always visible; no principal → restricted members hidden.
    assert!(requirements_visible(None, &ToolRequirements::NONE));
    assert!(!requirements_visible(None, &admin_req));
    assert!(!requirements_visible(None, &READ));
    // any_scopes: one held scope suffices.
    let any = ToolRequirements {
        any_scopes: &["mcp:admin", "mcp:read"],
        ..ToolRequirements::NONE
    };
    assert!(requirements_visible(Some(&plain), &any));
    assert!(!requirements_visible(Some(&admin), &any));
    // all_roles: every entry required.
    let all_roles = ToolRequirements {
        all_roles: &["admin", "auditor"],
        ..ToolRequirements::NONE
    };
    assert!(!requirements_visible(Some(&admin), &all_roles));
    let both = principal(&[], &["admin", "auditor"]).await;
    assert!(requirements_visible(Some(&both), &all_roles));
    // Scopes and roles combine with AND.
    let scoped_admin = ToolRequirements {
        scopes: &["mcp:read"],
        roles: &["admin"],
        ..ToolRequirements::NONE
    };
    assert!(!requirements_visible(Some(&admin), &scoped_admin));
    assert!(!requirements_visible(Some(&plain), &scoped_admin));
    let full = principal(&["mcp:read"], &["admin"]).await;
    assert!(requirements_visible(Some(&full), &scoped_admin));
}

// ── End-to-end over the wire ───────────────────────────────────────────────

fn alice_token() -> String {
    test_jwt()
        .token_builder("alice")
        .scopes(&["mcp:read", "mcp:write"])
        .roles(&["admin"])
        .build()
}

fn bob_token() -> String {
    test_jwt()
        .token_builder("bob")
        .scopes(&["mcp:read"])
        .build()
}

#[tokio::test]
async fn fully_scoped_caller_sees_and_calls_everything() {
    let router = secured_app().await;
    let token = alice_token();
    let session = initialize_auth(&router, "/mcp", &token).await;

    let list = tools_list_auth(&router, "/mcp", &session, &token).await;
    assert_eq!(
        tool_names(&list),
        [
            "admin_only",
            "flexible",
            "ping",
            "read_data",
            "whoami",
            "write_data"
        ]
    );

    let call = tools_call_auth(&router, "/mcp", &session, &token, "read_data", json!({})).await;
    assert_eq!(call["result"]["content"][0]["text"], "data");
    // The principal inserted by the layer reaches identity params…
    let call = tools_call_auth(&router, "/mcp", &session, &token, "whoami", json!({})).await;
    assert_eq!(call["result"]["content"][0]["text"], "alice");
    // …and satisfies the shared #[roles] guard.
    let call = tools_call_auth(&router, "/mcp", &session, &token, "admin_only", json!({})).await;
    assert_eq!(call["result"]["content"][0]["text"], "admin:alice");
}

#[tokio::test]
async fn tools_list_hides_what_the_caller_cannot_invoke() {
    let router = secured_app().await;
    let token = bob_token();
    let session = initialize_auth(&router, "/mcp", &token).await;
    let list = tools_list_auth(&router, "/mcp", &session, &token).await;
    // write_data (needs mcp:write), flexible (admin|write) and admin_only
    // (role) are filtered out for bob.
    assert_eq!(tool_names(&list), ["ping", "read_data", "whoami"]);
}

#[tokio::test]
async fn scope_denials_carry_agent_actionable_messages() {
    let router = secured_app().await;
    let token = bob_token();
    let session = initialize_auth(&router, "/mcp", &token).await;

    let call = tools_call_auth(&router, "/mcp", &session, &token, "write_data", json!({})).await;
    let error = &call["error"];
    assert_eq!(error["code"], -32600, "{call}");
    assert_eq!(error["data"], "forbidden");
    let msg = error["message"].as_str().unwrap();
    assert!(msg.contains("requires scope(s) `mcp:write`"), "{msg}");

    let call = tools_call_auth(&router, "/mcp", &session, &token, "flexible", json!({})).await;
    let msg = call["error"]["message"].as_str().unwrap();
    assert!(
        msg.contains("requires at least one of the scopes `mcp:admin, mcp:write`"),
        "{msg}"
    );
}

#[tokio::test]
async fn role_guard_denial_maps_to_forbidden() {
    let router = secured_app().await;
    let token = bob_token();
    let session = initialize_auth(&router, "/mcp", &token).await;
    // admin_only has no scope requirement — the denial comes from the shared
    // #[roles("admin")] guard (403 → Forbidden), proving guard reuse on MCP.
    let call = tools_call_auth(&router, "/mcp", &session, &token, "admin_only", json!({})).await;
    assert_eq!(call["error"]["code"], -32600, "{call}");
    assert_eq!(call["error"]["data"], "forbidden");
}

#[tokio::test]
async fn filter_members_false_lists_everything() {
    let router = secured_app_with(McpAuthConfig {
        filter_members: Some(false),
        ..offline_auth()
    })
    .await;
    let token = bob_token();
    let session = initialize_auth(&router, "/mcp", &token).await;
    let list = tools_list_auth(&router, "/mcp", &session, &token).await;
    assert_eq!(
        tool_names(&list),
        [
            "admin_only",
            "flexible",
            "ping",
            "read_data",
            "whoami",
            "write_data"
        ]
    );
    // Listing is not calling: invocation checks still apply.
    let call = tools_call_auth(&router, "/mcp", &session, &token, "write_data", json!({})).await;
    assert_eq!(call["error"]["data"], "forbidden");
}

#[tokio::test]
async fn unrestricted_tool_needs_no_scopes() {
    let router = secured_app().await;
    let token = test_jwt().token("carol", &[]);
    let session = initialize_auth(&router, "/mcp", &token).await;
    let call = tools_call_auth(&router, "/mcp", &session, &token, "ping", json!({})).await;
    assert_eq!(call["result"]["content"][0]["text"], "pong");
}

// ── Resources and prompts share the same authorization machinery ───────────

fn sorted_values(list: &serde_json::Value, array: &str, key: &str) -> Vec<String> {
    let mut out: Vec<String> = list[array]
        .as_array()
        .unwrap_or_else(|| panic!("no `{array}` array in {list}"))
        .iter()
        .map(|entry| entry[key].as_str().unwrap().to_string())
        .collect();
    out.sort();
    out
}

#[tokio::test]
async fn resource_and_prompt_lists_filter_by_scope() {
    let router = secured_app().await;

    // Alice holds mcp:write → sees everything.
    let token = alice_token();
    let session = initialize_auth(&router, "/mcp", &token).await;
    let resources = rpc_auth(
        &router,
        "/mcp",
        &session,
        &token,
        "resources/list",
        json!({}),
    )
    .await;
    assert_eq!(
        sorted_values(&resources["result"], "resources", "uri"),
        ["r2e://secured/info", "r2e://secured/report"]
    );
    let prompts = rpc_auth(&router, "/mcp", &session, &token, "prompts/list", json!({})).await;
    assert_eq!(
        sorted_values(&prompts["result"], "prompts", "name"),
        ["howto", "write_recipe"]
    );

    // Bob (mcp:read only) → the write-gated members are hidden.
    let token = bob_token();
    let session = initialize_auth(&router, "/mcp", &token).await;
    let resources = rpc_auth(
        &router,
        "/mcp",
        &session,
        &token,
        "resources/list",
        json!({}),
    )
    .await;
    assert_eq!(
        sorted_values(&resources["result"], "resources", "uri"),
        ["r2e://secured/info"]
    );
    let prompts = rpc_auth(&router, "/mcp", &session, &token, "prompts/list", json!({})).await;
    assert_eq!(
        sorted_values(&prompts["result"], "prompts", "name"),
        ["howto"]
    );
}

#[tokio::test]
async fn scoped_resource_and_prompt_denials_are_json_rpc_errors() {
    let router = secured_app().await;
    let token = bob_token();
    let session = initialize_auth(&router, "/mcp", &token).await;

    // Unlike tools, resources/prompts have no in-result error plane: the
    // scope denial arrives as a JSON-RPC error with the same `forbidden`
    // marker, naming the member by its family.
    let read = rpc_auth(
        &router,
        "/mcp",
        &session,
        &token,
        "resources/read",
        json!({ "uri": "r2e://secured/report" }),
    )
    .await;
    assert_eq!(read["error"]["code"], -32600, "{read}");
    assert_eq!(read["error"]["data"], "forbidden");
    let msg = read["error"]["message"].as_str().unwrap();
    assert!(
        msg.contains("resource `report` requires scope(s) `mcp:write`"),
        "{msg}"
    );

    let get = rpc_auth(
        &router,
        "/mcp",
        &session,
        &token,
        "prompts/get",
        json!({ "name": "write_recipe", "arguments": {} }),
    )
    .await;
    assert_eq!(get["error"]["code"], -32600, "{get}");
    assert_eq!(get["error"]["data"], "forbidden");
    let msg = get["error"]["message"].as_str().unwrap();
    assert!(
        msg.contains("prompt `write_recipe` requires scope(s) `mcp:write`"),
        "{msg}"
    );

    // With the right scope both go through.
    let token = alice_token();
    let session = initialize_auth(&router, "/mcp", &token).await;
    let read = rpc_auth(
        &router,
        "/mcp",
        &session,
        &token,
        "resources/read",
        json!({ "uri": "r2e://secured/report" }),
    )
    .await;
    assert_eq!(read["result"]["contents"][0]["text"], "confidential report");
    let get = rpc_auth(
        &router,
        "/mcp",
        &session,
        &token,
        "prompts/get",
        json!({ "name": "write_recipe", "arguments": {} }),
    )
    .await;
    assert_eq!(
        get["result"]["messages"][0]["content"]["text"],
        "Call `write_data` with the payload."
    );
}
