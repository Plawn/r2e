//! Per-session member lists: groups (`#[mcp_routes(group, opt_in)]`), the
//! `McpSession` member parameter, session-private `Dynamic*` members,
//! `McpServer::session_init`, the `McpSessions` bean, and the
//! `list_changed` capability/notifications.

use http_body_util::BodyExt;
use r2e_core::http::{Body, Request, Router, StatusCode};
use r2e_core::prelude::*;
use r2e_core::AppBuilder;
use r2e_mcp::{
    AppBuilderMcpExt, DynamicPrompt, DynamicResource, DynamicTool, McpError, McpServer, McpSession,
    McpSessionInit, McpSessions, Params, PromptCall, ResourceCall, SessionInit, SessionToolset,
    ToolCall,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use tower::ServiceExt;

use crate::support::{self, initialize, tools_call, tools_list};

// ── Services ───────────────────────────────────────────────────────────────

#[derive(Deserialize, JsonSchema, ObjectParams)]
pub struct NameIn {
    pub name: String,
}

#[derive(Deserialize, JsonSchema, ObjectParams)]
pub struct EchoIn {
    pub text: String,
}

#[controller]
pub struct Toolbox;

#[mcp_routes]
impl Toolbox {
    /// Always visible.
    #[tool]
    async fn ping(&self) -> &'static str {
        "pong"
    }

    /// Reveal the git toolset to this session.
    #[tool]
    async fn enable_git(&self, session: McpSession) -> Result<&'static str, McpError> {
        session.enable_group("git")?;
        Ok("git enabled")
    }

    /// Hide the git toolset again.
    #[tool]
    async fn disable_git(&self, session: McpSession) -> Result<&'static str, McpError> {
        session.disable_group("git")?;
        Ok("git disabled")
    }

    /// Create a session-private echo tool named `name`.
    #[tool]
    async fn add_echo(
        &self,
        Params(p): Params<NameIn>,
        session: McpSession,
    ) -> Result<&'static str, McpError> {
        session.add_tool(
            DynamicTool::new(p.name)
                .description("Echo the text back")
                .handler(|p: EchoIn, _call: ToolCall| async move { p.text }),
        )?;
        Ok("added")
    }

    /// Remove the session-private tool `name`.
    #[tool]
    async fn remove_echo(
        &self,
        Params(p): Params<NameIn>,
        session: McpSession,
    ) -> Result<String, McpError> {
        Ok(session.remove_tool(&p.name)?.to_string())
    }

    /// Add a private resource and prompt.
    #[tool]
    async fn add_extras(&self, session: McpSession) -> Result<&'static str, McpError> {
        session.apply(
            SessionToolset::new()
                .resource(
                    DynamicResource::new("r2e://session/notes", "notes")
                        .mime_type("text/plain")
                        .handler(|_call: ResourceCall| async { "private notes" }),
                )
                .prompt(
                    DynamicPrompt::new("review")
                        .description("Review the session notes")
                        .handler(|p: EchoIn, _call: PromptCall| async move {
                            format!("Review: {}", p.text)
                        }),
                ),
        )?;
        Ok("added")
    }
}

#[controller]
pub struct GitTools;

#[mcp_routes(group = "git", opt_in)]
impl GitTools {
    /// Working tree status.
    #[tool]
    async fn git_status(&self) -> &'static str {
        "clean"
    }

    /// A commit message template.
    #[prompt]
    async fn commit_message(&self) -> &'static str {
        "Write a conventional commit message."
    }
}

#[controller]
pub struct AdminTools;

#[mcp_routes]
impl AdminTools {
    /// Per-member group, visible by default.
    #[tool(group = "admin")]
    async fn stats(&self) -> &'static str {
        "42"
    }
}

async fn dynamic_app_with(plugin: McpServer) -> (Router, McpSessions) {
    let app = AppBuilder::new()
        .plugin(plugin)
        .build_state()
        .await
        .register_mcp_service::<Toolbox>()
        .register_mcp_service::<GitTools>()
        .register_mcp_service::<AdminTools>();
    let sessions = app
        .bean_context()
        .try_get::<McpSessions>()
        .expect("McpServer provides McpSessions");
    (app.build(), sessions)
}

async fn dynamic_app() -> (Router, McpSessions) {
    dynamic_app_with(McpServer::new()).await
}

fn text_of(msg: &Value) -> &str {
    msg["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no text content in {msg}"))
}

// ── Groups ─────────────────────────────────────────────────────────────────

#[r2e_core::test]
async fn opt_in_group_is_hidden_and_default_group_is_visible() {
    let (router, _) = dynamic_app().await;
    let session = initialize(&router, "/mcp").await;
    let tools = support::tool_names(&tools_list(&router, "/mcp", &session).await);
    assert!(tools.contains(&"stats".to_string()), "{tools:?}");
    assert!(!tools.contains(&"git_status".to_string()), "{tools:?}");
    let prompts = support::names(
        &support::prompts_list(&router, "/mcp", &session).await,
        "prompts",
    );
    assert!(
        !prompts.contains(&"commit_message".to_string()),
        "{prompts:?}"
    );
}

#[r2e_core::test]
async fn hidden_member_answers_like_an_unknown_one() {
    let (router, _) = dynamic_app().await;
    let session = initialize(&router, "/mcp").await;
    let msg = tools_call(&router, "/mcp", &session, "git_status", json!({})).await;
    assert_eq!(msg["error"]["code"], -32601, "{msg}");
    let unknown = tools_call(&router, "/mcp", &session, "nope", json!({})).await;
    assert_eq!(unknown["error"]["code"], msg["error"]["code"], "{unknown}");
    let prompt = support::prompts_get(&router, "/mcp", &session, "commit_message", json!({})).await;
    assert!(prompt.get("error").is_some(), "{prompt}");
}

#[r2e_core::test]
async fn enabling_a_group_changes_only_that_session() {
    let (router, _) = dynamic_app().await;
    let a = initialize(&router, "/mcp").await;
    let b = initialize(&router, "/mcp").await;

    let enabled = tools_call(&router, "/mcp", &a, "enable_git", json!({})).await;
    assert_eq!(text_of(&enabled), "git enabled", "{enabled}");

    let tools_a = support::tool_names(&tools_list(&router, "/mcp", &a).await);
    assert!(tools_a.contains(&"git_status".to_string()), "{tools_a:?}");
    let called = tools_call(&router, "/mcp", &a, "git_status", json!({})).await;
    assert_eq!(text_of(&called), "clean", "{called}");
    let prompts_a = support::names(&support::prompts_list(&router, "/mcp", &a).await, "prompts");
    assert!(
        prompts_a.contains(&"commit_message".to_string()),
        "{prompts_a:?}"
    );

    let tools_b = support::tool_names(&tools_list(&router, "/mcp", &b).await);
    assert!(!tools_b.contains(&"git_status".to_string()), "{tools_b:?}");
    let refused = tools_call(&router, "/mcp", &b, "git_status", json!({})).await;
    assert_eq!(refused["error"]["code"], -32601, "{refused}");

    tools_call(&router, "/mcp", &a, "disable_git", json!({})).await;
    let tools_a = support::tool_names(&tools_list(&router, "/mcp", &a).await);
    assert!(!tools_a.contains(&"git_status".to_string()), "{tools_a:?}");
}

#[r2e_core::test]
async fn capabilities_advertise_list_changed_when_lists_are_dynamic() {
    let (router, _) = dynamic_app().await;
    let response = support::post(&router, "/mcp", None, &support::initialize_body()).await;
    let capabilities = &response.result()["capabilities"];
    assert_eq!(capabilities["tools"]["listChanged"], true, "{capabilities}");
    assert_eq!(
        capabilities["prompts"]["listChanged"], true,
        "{capabilities}"
    );
    assert_eq!(
        capabilities["resources"]["listChanged"], true,
        "{capabilities}"
    );
}

// ── Session-private members ────────────────────────────────────────────────

#[r2e_core::test]
async fn private_tool_is_served_to_its_session_only() {
    let (router, _) = dynamic_app().await;
    let a = initialize(&router, "/mcp").await;
    let b = initialize(&router, "/mcp").await;

    let added = tools_call(&router, "/mcp", &a, "add_echo", json!({ "name": "echo" })).await;
    assert_eq!(text_of(&added), "added", "{added}");

    let list = tools_list(&router, "/mcp", &a).await;
    let echo = support::tool(&list, "echo");
    assert_eq!(echo["description"], "Echo the text back");
    assert_eq!(
        echo["inputSchema"]["properties"]["text"]["type"], "string",
        "{echo}"
    );
    let echoed = tools_call(&router, "/mcp", &a, "echo", json!({ "text": "hi" })).await;
    assert_eq!(text_of(&echoed), "hi", "{echoed}");

    assert!(
        !support::tool_names(&tools_list(&router, "/mcp", &b).await).contains(&"echo".to_string())
    );
    let refused = tools_call(&router, "/mcp", &b, "echo", json!({ "text": "hi" })).await;
    assert_eq!(refused["error"]["code"], -32601, "{refused}");

    let removed = tools_call(
        &router,
        "/mcp",
        &a,
        "remove_echo",
        json!({ "name": "echo" }),
    )
    .await;
    assert_eq!(text_of(&removed), "true", "{removed}");
    let gone = tools_call(&router, "/mcp", &a, "echo", json!({ "text": "hi" })).await;
    assert_eq!(gone["error"]["code"], -32601, "{gone}");
}

#[r2e_core::test]
async fn private_tool_cannot_shadow_a_catalog_tool_even_hidden() {
    let (router, _) = dynamic_app().await;
    let session = initialize(&router, "/mcp").await;
    for name in ["ping", "git_status"] {
        let msg = tools_call(
            &router,
            "/mcp",
            &session,
            "add_echo",
            json!({ "name": name }),
        )
        .await;
        assert_eq!(msg["result"]["isError"], true, "{msg}");
        assert!(text_of(&msg).contains("already exists"), "{msg}");
    }
    tools_call(
        &router,
        "/mcp",
        &session,
        "add_echo",
        json!({ "name": "echo" }),
    )
    .await;
    let twice = tools_call(
        &router,
        "/mcp",
        &session,
        "add_echo",
        json!({ "name": "echo" }),
    )
    .await;
    assert_eq!(twice["result"]["isError"], true, "{twice}");
}

#[r2e_core::test]
async fn private_resource_and_prompt_are_served() {
    let (router, _) = dynamic_app().await;
    let session = initialize(&router, "/mcp").await;
    tools_call(&router, "/mcp", &session, "add_extras", json!({})).await;

    let resources = support::resources_list(&router, "/mcp", &session).await;
    assert_eq!(
        support::resource(&resources, "r2e://session/notes")["mimeType"],
        "text/plain"
    );
    let read = support::resources_read(&router, "/mcp", &session, "r2e://session/notes").await;
    assert_eq!(
        read["result"]["contents"][0]["text"], "private notes",
        "{read}"
    );

    let prompts = support::prompts_list(&router, "/mcp", &session).await;
    let review = support::prompt(&prompts, "review");
    assert_eq!(review["arguments"][0]["name"], "text", "{review}");
    let got =
        support::prompts_get(&router, "/mcp", &session, "review", json!({ "text": "x" })).await;
    assert_eq!(
        got["result"]["messages"][0]["content"]["text"], "Review: x",
        "{got}"
    );

    let other = initialize(&router, "/mcp").await;
    let read = support::resources_read(&router, "/mcp", &other, "r2e://session/notes").await;
    assert!(read.get("error").is_some(), "{read}");
}

// ── Stateless serving ──────────────────────────────────────────────────────

#[r2e_core::test]
#[should_panic(expected = "mcp.stateless")]
async fn a_session_member_under_stateless_aborts_startup() {
    let _ = dynamic_app_with(McpServer::new().stateless(true)).await;
}

#[r2e_core::test]
async fn stateless_serving_recomputes_session_init_per_request() {
    let router = AppBuilder::new()
        .provide(TierToolsets)
        .plugin(
            McpServer::new()
                .stateless(true)
                .json_response(true)
                .session_init::<TierToolsets>(),
        )
        .build_state()
        .await
        .register_mcp_service::<GitTools>()
        .register_mcp_service::<AdminTools>()
        .build();
    let list = |tier: &'static str| {
        let router = router.clone();
        async move {
            let response = support::post_with_headers(
                &router,
                "/mcp",
                None,
                &[("x-tier", tier)],
                &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
            )
            .await;
            support::tool_names(response.result())
        }
    };
    assert_eq!(list("pro").await, vec!["git_status".to_string()]);
    assert!(list("free").await.is_empty());
}

// ── session_init ───────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct TierToolsets;

impl McpSessionInit for TierToolsets {
    async fn init(&self, session: &SessionInit) -> Result<SessionToolset, McpError> {
        let pro = session.header("x-tier") == Some("pro");
        Ok(SessionToolset::new().enable_if(pro, "git").disable("admin"))
    }
}

#[r2e_core::test]
async fn session_init_shapes_the_list_from_the_opening_request() {
    let app = AppBuilder::new()
        .provide(TierToolsets)
        .plugin(McpServer::new().session_init::<TierToolsets>())
        .build_state()
        .await
        .register_mcp_service::<Toolbox>()
        .register_mcp_service::<GitTools>()
        .register_mcp_service::<AdminTools>()
        .build();

    let pro = support::initialize_with_headers(&app, "/mcp", &[("x-tier", "pro")]).await;
    let tools = support::tool_names(&tools_list(&app, "/mcp", &pro).await);
    assert!(tools.contains(&"git_status".to_string()), "{tools:?}");
    assert!(!tools.contains(&"stats".to_string()), "{tools:?}");

    let free = support::initialize_with_headers(&app, "/mcp", &[]).await;
    let tools = support::tool_names(&tools_list(&app, "/mcp", &free).await);
    assert!(!tools.contains(&"git_status".to_string()), "{tools:?}");
    assert!(!tools.contains(&"stats".to_string()), "{tools:?}");
}

#[r2e_core::test]
#[should_panic(expected = "no `")]
async fn session_init_without_its_bean_aborts_startup() {
    let _ = AppBuilder::new()
        .plugin(McpServer::new().session_init::<TierToolsets>())
        .build_state()
        .await
        .register_mcp_service::<Toolbox>()
        .build();
}

// ── McpSessions + notifications ────────────────────────────────────────────

#[r2e_core::test]
async fn sessions_bean_reaches_live_sessions() {
    let (router, sessions) = dynamic_app().await;
    let session = initialize(&router, "/mcp").await;
    tools_list(&router, "/mcp", &session).await;
    assert_eq!(sessions.len(), 1);

    for live in sessions.all() {
        live.enable_group("git").unwrap();
    }
    let tools = support::tool_names(&tools_list(&router, "/mcp", &session).await);
    assert!(tools.contains(&"git_status".to_string()), "{tools:?}");
}

#[r2e_core::test]
async fn enabling_a_group_notifies_tools_list_changed() {
    let (router, _) = dynamic_app().await;
    let session = initialize(&router, "/mcp").await;

    let stream = router
        .clone()
        .oneshot(
            Request::get("/mcp")
                .header("host", "localhost")
                .header("accept", "text/event-stream")
                .header("mcp-session-id", &session)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stream.status(), StatusCode::OK);
    let mut body = stream.into_body();

    tools_call(&router, "/mcp", &session, "enable_git", json!({})).await;

    let seen = r2e_core::rt::timeout(std::time::Duration::from_secs(2), async {
        let mut seen = String::new();
        while !(seen.contains("notifications/tools/list_changed")
            && seen.contains("notifications/prompts/list_changed"))
        {
            let frame = body.frame().await.expect("SSE stream ended").unwrap();
            if let Some(data) = frame.data_ref() {
                seen.push_str(&String::from_utf8_lossy(data));
            }
        }
        seen
    })
    .await
    .expect("list_changed notifications timed out");
    // Resources did not change: no resources notification.
    assert!(
        !seen.contains("notifications/resources/list_changed"),
        "{seen}"
    );
}
