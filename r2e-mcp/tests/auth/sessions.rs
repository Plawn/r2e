//! Per-session member lists under OAuth: a session is bound to the principal
//! that opened it, private `Dynamic*` members keep their scope/role gates,
//! and `session_init` / `McpSessions` see the authenticated identity.

use r2e_core::http::Router;
use r2e_core::prelude::*;
use r2e_core::AppBuilder;
use r2e_mcp::{
    AppBuilderMcpExt, DynamicTool, McpError, McpServer, McpSession, McpSessionInit, McpSessions,
    SessionInit, SessionToolset, ToolCall,
};
use serde_json::{json, Value};

use crate::fixtures::{
    initialize_auth, offline_auth, pinned, post_auth, test_jwt, tool_names, tools_call_auth,
    tools_list_auth,
};

// ── Services ───────────────────────────────────────────────────────────────

#[controller]
pub struct SessionTools;

#[mcp_routes]
impl SessionTools {
    #[tool]
    async fn ping(&self) -> &'static str {
        "pong"
    }

    /// Adds a scope-gated and a role-gated private tool to this session.
    #[tool]
    async fn add_private(&self, session: McpSession) -> Result<&'static str, McpError> {
        session.add_tool(
            DynamicTool::new("private_read")
                .scopes(&["mcp:read"])
                .handler(|_call: ToolCall| async { "secret" }),
        )?;
        session.add_tool(
            DynamicTool::new("private_admin")
                .roles(&["admin"])
                .handler(|_call: ToolCall| async { "admin secret" }),
        )?;
        Ok("added")
    }
}

#[controller]
pub struct OpsTools;

#[mcp_routes(group = "ops", opt_in)]
impl OpsTools {
    #[tool]
    async fn restart(&self) -> &'static str {
        "restarted"
    }
}

#[derive(Clone)]
pub struct RoleToolsets;

impl McpSessionInit for RoleToolsets {
    async fn init(&self, session: &SessionInit) -> Result<SessionToolset, McpError> {
        Ok(SessionToolset::new().enable_if(session.has_role("ops"), "ops"))
    }
}

async fn app_with(plugin: McpServer) -> (Router, McpSessions) {
    let app = AppBuilder::new()
        .provide(RoleToolsets)
        .plugin(
            plugin
                .with_auth(offline_auth())
                .with_token_validator(pinned(&test_jwt())),
        )
        .build_state()
        .await
        .register_mcp_service::<SessionTools>()
        .register_mcp_service::<OpsTools>();
    let sessions = app
        .bean_context()
        .try_get::<McpSessions>()
        .expect("McpServer provides McpSessions");
    (app.build(), sessions)
}

fn token(sub: &str, scopes: &[&str], roles: &[&str]) -> String {
    test_jwt()
        .token_builder(sub)
        .scopes(scopes)
        .roles(roles)
        .build()
}

fn error_message(msg: &Value) -> String {
    msg["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

// ── Session ↔ principal binding ────────────────────────────────────────────

#[r2e_core::test]
async fn replayed_session_id_with_another_principal_is_refused() {
    let (router, _) = app_with(McpServer::new()).await;
    let alice = token("alice", &[], &[]);
    let bob = token("bob", &[], &[]);
    let session = initialize_auth(&router, "/mcp", &alice).await;

    let ok = tools_call_auth(&router, "/mcp", &session, &alice, "ping", json!({})).await;
    assert_eq!(ok["result"]["content"][0]["text"], "pong", "{ok}");

    let response = post_auth(
        &router,
        "/mcp",
        Some(&session),
        &bob,
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    )
    .await;
    let msg = response.message();
    assert!(
        msg.get("result").is_none(),
        "bob must not list alice's session: {msg}"
    );
    assert!(
        error_message(msg).contains("belongs to another principal"),
        "{msg}"
    );

    let call = tools_call_auth(&router, "/mcp", &session, &bob, "ping", json!({})).await;
    assert!(call.get("result").is_none(), "{call}");

    // The owner is unaffected by the rejected replay.
    let still = tools_call_auth(&router, "/mcp", &session, &alice, "ping", json!({})).await;
    assert_eq!(still["result"]["content"][0]["text"], "pong", "{still}");
}

// ── Private members keep their gates ───────────────────────────────────────

#[r2e_core::test]
async fn private_members_are_filtered_and_enforced_by_scopes_and_roles() {
    let (router, _) = app_with(McpServer::new()).await;
    let plain = token("alice", &[], &[]);
    let session = initialize_auth(&router, "/mcp", &plain).await;
    let added = tools_call_auth(&router, "/mcp", &session, &plain, "add_private", json!({})).await;
    assert_eq!(added["result"]["content"][0]["text"], "added", "{added}");

    let tools = tool_names(&tools_list_auth(&router, "/mcp", &session, &plain).await);
    assert!(!tools.contains(&"private_read".to_string()), "{tools:?}");
    assert!(!tools.contains(&"private_admin".to_string()), "{tools:?}");
    let denied =
        tools_call_auth(&router, "/mcp", &session, &plain, "private_read", json!({})).await;
    assert!(
        denied.get("result").is_none() || denied["result"]["isError"] == true,
        "{denied}"
    );
    let denied = tools_call_auth(
        &router,
        "/mcp",
        &session,
        &plain,
        "private_admin",
        json!({}),
    )
    .await;
    assert!(
        denied.get("result").is_none() || denied["result"]["isError"] == true,
        "{denied}"
    );

    // Same subject with the scope and role: the members appear and answer.
    let strong = token("alice", &["mcp:read"], &["admin"]);
    let tools = tool_names(&tools_list_auth(&router, "/mcp", &session, &strong).await);
    assert!(tools.contains(&"private_read".to_string()), "{tools:?}");
    assert!(tools.contains(&"private_admin".to_string()), "{tools:?}");
    let read = tools_call_auth(
        &router,
        "/mcp",
        &session,
        &strong,
        "private_read",
        json!({}),
    )
    .await;
    assert_eq!(read["result"]["content"][0]["text"], "secret", "{read}");
    let admin = tools_call_auth(
        &router,
        "/mcp",
        &session,
        &strong,
        "private_admin",
        json!({}),
    )
    .await;
    assert_eq!(
        admin["result"]["content"][0]["text"], "admin secret",
        "{admin}"
    );
}

// ── session_init sees the principal ────────────────────────────────────────

#[r2e_core::test]
async fn session_init_shapes_the_list_from_the_principal_roles() {
    let (router, _) = app_with(McpServer::new().session_init::<RoleToolsets>()).await;

    let ops = token("olive", &[], &["ops"]);
    let session = initialize_auth(&router, "/mcp", &ops).await;
    let tools = tool_names(&tools_list_auth(&router, "/mcp", &session, &ops).await);
    assert!(tools.contains(&"restart".to_string()), "{tools:?}");

    let dev = token("dave", &[], &[]);
    let session = initialize_auth(&router, "/mcp", &dev).await;
    let tools = tool_names(&tools_list_auth(&router, "/mcp", &session, &dev).await);
    assert!(!tools.contains(&"restart".to_string()), "{tools:?}");
    let hidden = tools_call_auth(&router, "/mcp", &session, &dev, "restart", json!({})).await;
    assert_eq!(hidden["error"]["code"], -32601, "{hidden}");
}

// ── McpSessions by subject ─────────────────────────────────────────────────

#[r2e_core::test]
async fn sessions_bean_finds_sessions_by_subject() {
    let (router, sessions) = app_with(McpServer::new()).await;
    let alice = token("alice", &[], &[]);
    let bob = token("bob", &[], &[]);
    let a1 = initialize_auth(&router, "/mcp", &alice).await;
    let _a2 = initialize_auth(&router, "/mcp", &alice).await;
    let b = initialize_auth(&router, "/mcp", &bob).await;

    // Registered at the handshake: no request needed first.
    assert_eq!(sessions.for_subject("alice").len(), 2);
    assert_eq!(sessions.for_subject("bob").len(), 1);
    assert!(sessions.for_subject("carol").is_empty());

    // Revealing a group to bob's sessions only.
    for session in sessions.for_subject("bob") {
        assert_eq!(session.subject(), Some("bob"));
        session.enable_group("ops").unwrap();
    }
    let tools = tool_names(&tools_list_auth(&router, "/mcp", &b, &bob).await);
    assert!(tools.contains(&"restart".to_string()), "{tools:?}");
    let tools = tool_names(&tools_list_auth(&router, "/mcp", &a1, &alice).await);
    assert!(!tools.contains(&"restart".to_string()), "{tools:?}");
}
