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

#[r2e_core::test]
async fn dynamic_preserves_raw_arguments() {
    let (router, sessions) = dynamic_app().await;
    let sid = initialize(&router, "/mcp").await;
    sessions.all()[0]
        .add_tool(
            DynamicTool::new("raw_args")
                .handler(|_p: EchoIn, call: ToolCall| async move { call.arguments.to_string() }),
        )
        .unwrap();
    let result = tools_call(&router, "/mcp", &sid, "raw_args", json!({"text":"hello"})).await;
    assert_eq!(text_of(&result), r#"{"text":"hello"}"#);
    sessions.all()[0]
        .add_prompt(
            DynamicPrompt::new("raw_prompt")
                .handler(|_p: EchoIn, call: PromptCall| async move { call.arguments.to_string() }),
        )
        .unwrap();
    let result =
        support::prompts_get(&router, "/mcp", &sid, "raw_prompt", json!({"text":"hello"})).await;
    assert_eq!(
        result["result"]["messages"][0]["content"]["text"],
        r#"{"text":"hello"}"#
    );
}

#[r2e_core::test]
async fn external_mutations_advertise_capabilities() {
    let app = AppBuilder::new()
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_mcp_service::<AdminTools>();
    let sessions = app.bean_context().try_get::<McpSessions>().unwrap();
    let router = app.build();
    let init = support::post(&router, "/mcp", None, &support::initialize_body()).await;
    let caps = &init.result()["capabilities"];
    sessions.all()[0]
        .add_resource(
            DynamicResource::new("r2e://new", "new").handler(|_call: ResourceCall| async { "new" }),
        )
        .unwrap();
    for family in ["tools", "resources", "prompts"] {
        assert_eq!(caps[family]["listChanged"], true, "{caps}");
    }
    assert!(caps.get("completions").is_some(), "{caps}");
}

#[derive(Clone)]
struct ResourceSubscriptionInit;
impl McpSessionInit for ResourceSubscriptionInit {
    async fn init(&self, s: &SessionInit) -> Result<SessionToolset, McpError> {
        assert_eq!(s.header("x-resource-view"), Some("private"));
        Ok(SessionToolset::new()
            .resource(
                DynamicResource::new("r2e://private", "private")
                    .handler(|_call: ResourceCall| async { "private" }),
            )
            .resource(
                DynamicResource::new("r2e://admin", "admin")
                    .roles(&["admin"])
                    .handler(|_call: ResourceCall| async { "admin" }),
            ))
    }
}

#[r2e_core::test]
async fn modern_subscription_runs_init() {
    let updates = r2e_mcp::McpResourceUpdates::default();
    let router = AppBuilder::new()
        .provide(ResourceSubscriptionInit)
        .plugin(
            McpServer::new()
                .session_init::<ResourceSubscriptionInit>()
                .with_resource_updates(updates.clone()),
        )
        .build_state()
        .await
        .register_mcp_service::<AdminTools>()
        .build();
    let req = json!({"jsonrpc":"2.0","id":1,"method":"subscriptions/listen",
        "params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}},
            "notifications":{"resourceSubscriptions":["r2e://private", "r2e://admin", "r2e://unknown"]}}});
    let request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("x-resource-view", "private")
        .header("mcp-protocol-version", "2026-07-28")
        .header("mcp-method", "subscriptions/listen")
        .body(Body::from(req.to_string()))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    if response.status() != StatusCode::OK {
        panic!(
            "{}",
            String::from_utf8_lossy(&response.into_body().collect().await.unwrap().to_bytes())
        );
    }
    let mut body = response.into_body();
    let acknowledged = r2e_core::rt::timeout(std::time::Duration::from_secs(2), async {
        let mut raw = String::new();
        while let Some(frame) = body.frame().await {
            if let Ok(data) = frame.unwrap().into_data() {
                raw.push_str(&String::from_utf8_lossy(&data));
                if raw.contains("notifications/subscriptions/acknowledged") {
                    return raw;
                }
            }
        }
        raw
    })
    .await
    .unwrap();
    assert!(acknowledged.contains("r2e://private"), "{acknowledged}");
    // Wait for the initialized listener, then put denied/unknown updates ahead
    // of the visible one. Only the visible URI may reach the client.
    r2e_core::rt::timeout(std::time::Duration::from_secs(2), async {
        while updates.notify("r2e://unknown") == 0 {
            r2e_core::rt::yield_now().await;
        }
    })
    .await
    .unwrap();
    updates.notify("r2e://admin");
    updates.notify("r2e://private");
    let received = r2e_core::rt::timeout(std::time::Duration::from_secs(2), async {
        let mut raw = String::new();
        loop {
            let frame = body.frame().await.expect("subscription ended").unwrap();
            if let Some(data) = frame.data_ref() {
                raw.push_str(&String::from_utf8_lossy(data));
                if raw.contains("r2e://private") {
                    return raw;
                }
            }
        }
    })
    .await
    .expect("initialized resource update was not delivered");
    assert!(
        received.contains("notifications/resources/updated"),
        "{received}"
    );
    assert!(!received.contains("r2e://admin"), "{received}");
    assert!(!received.contains("r2e://unknown"), "{received}");
}

fn legacy_subscribe_body(uri: &str) -> Value {
    json!({"jsonrpc":"2.0","id":10,"method":"resources/subscribe","params":{"uri":uri}})
}

#[r2e_core::test]
async fn legacy_subscribe_refuses_unauthorized_like_unknown() {
    let (router, sessions) = dynamic_app().await;
    let sid = initialize(&router, "/mcp").await;
    let session = &sessions.all()[0];
    session
        .add_resource(
            DynamicResource::new("r2e://public", "public")
                .handler(|_call: ResourceCall| async { "public" }),
        )
        .unwrap();
    session
        .add_resource(
            DynamicResource::new("r2e://admin", "admin")
                .roles(&["admin"])
                .handler(|_call: ResourceCall| async { "admin" }),
        )
        .unwrap();

    let public = support::post(
        &router,
        "/mcp",
        Some(&sid),
        &legacy_subscribe_body("r2e://public"),
    )
    .await;
    assert!(
        public.message().get("error").is_none(),
        "{}",
        public.raw_body
    );

    let admin = support::post(
        &router,
        "/mcp",
        Some(&sid),
        &legacy_subscribe_body("r2e://admin"),
    )
    .await;
    let unknown = support::post(
        &router,
        "/mcp",
        Some(&sid),
        &legacy_subscribe_body("r2e://unknown"),
    )
    .await;
    let (admin, unknown) = (&admin.message()["error"], &unknown.message()["error"]);
    assert_eq!(admin["code"], unknown["code"], "{admin} vs {unknown}");
    assert_eq!(admin["message"], "unknown resource: r2e://admin");
    assert_eq!(unknown["message"], "unknown resource: r2e://unknown");
}

#[r2e_core::test]
async fn legacy_subscription_stops_after_removal() {
    let updates = r2e_mcp::McpResourceUpdates::default();
    let (router, sessions) =
        dynamic_app_with(McpServer::new().with_resource_updates(updates.clone())).await;
    let sid = initialize(&router, "/mcp").await;
    let session = &sessions.all()[0];
    for uri in ["r2e://gone", "r2e://kept"] {
        session
            .add_resource(
                DynamicResource::new(uri, uri).handler(|_call: ResourceCall| async { "x" }),
            )
            .unwrap();
        let sub = support::post(&router, "/mcp", Some(&sid), &legacy_subscribe_body(uri)).await;
        assert!(sub.message().get("error").is_none(), "{}", sub.raw_body);
    }
    let stream = router
        .clone()
        .oneshot(
            Request::get("/mcp")
                .header("host", "localhost")
                .header("accept", "text/event-stream")
                .header("mcp-session-id", &sid)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stream.status(), StatusCode::OK);
    let mut body = stream.into_body();
    assert!(session.remove_resource("r2e://gone").unwrap());
    // The removed URI is published first; only the kept one may arrive.
    updates.notify("r2e://gone");
    updates.notify("r2e://kept");
    let received = r2e_core::rt::timeout(std::time::Duration::from_secs(2), async {
        let mut raw = String::new();
        loop {
            let frame = body.frame().await.expect("SSE stream ended").unwrap();
            if let Some(data) = frame.data_ref() {
                raw.push_str(&String::from_utf8_lossy(data));
                if raw.contains("r2e://kept") {
                    return raw;
                }
            }
        }
    })
    .await
    .expect("kept resource update was not delivered");
    assert!(!received.contains("r2e://gone"), "{received}");
}

#[r2e_core::test]
async fn legacy_subscription_is_invalidated_on_replacement() {
    let updates = r2e_mcp::McpResourceUpdates::default();
    let (router, sessions) =
        dynamic_app_with(McpServer::new().with_resource_updates(updates.clone())).await;
    let sid = initialize(&router, "/mcp").await;
    let session = &sessions.all()[0];
    for uri in ["r2e://gone", "r2e://kept"] {
        session
            .add_resource(
                DynamicResource::new(uri, uri).handler(|_call: ResourceCall| async { "x" }),
            )
            .unwrap();
        let sub = support::post(&router, "/mcp", Some(&sid), &legacy_subscribe_body(uri)).await;
        assert!(sub.message().get("error").is_none(), "{}", sub.raw_body);
    }
    let stream = router
        .clone()
        .oneshot(
            Request::get("/mcp")
                .header("host", "localhost")
                .header("accept", "text/event-stream")
                .header("mcp-session-id", &sid)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stream.status(), StatusCode::OK);
    let mut body = stream.into_body();
    assert!(session.remove_resource("r2e://gone").unwrap());
    session
        .add_resource(
            DynamicResource::new("r2e://gone", "restricted replacement")
                .roles(&["admin"])
                .handler(|_call: ResourceCall| async { "restricted" }),
        )
        .unwrap();
    // Replacing a public resource with an admin-only one must revoke delivery.
    updates.notify("r2e://gone");
    updates.notify("r2e://kept");
    let received = r2e_core::rt::timeout(std::time::Duration::from_secs(2), async {
        let mut raw = String::new();
        loop {
            let frame = body.frame().await.expect("SSE stream ended").unwrap();
            if let Some(data) = frame.data_ref() {
                raw.push_str(&String::from_utf8_lossy(data));
                if raw.contains("r2e://kept") {
                    return raw;
                }
            }
        }
    })
    .await
    .expect("kept resource update was not delivered");
    assert!(!received.contains("r2e://gone"), "{received}");
    let denied = support::post(
        &router,
        "/mcp",
        Some(&sid),
        &legacy_subscribe_body("r2e://gone"),
    )
    .await;
    assert_eq!(denied.message()["error"]["code"], -32002);

    assert!(session.remove_resource("r2e://gone").unwrap());
    session
        .add_resource(
            DynamicResource::new("r2e://gone", "public replacement")
                .handler(|_call: ResourceCall| async { "public" }),
        )
        .unwrap();
    let subscribed = support::post(
        &router,
        "/mcp",
        Some(&sid),
        &legacy_subscribe_body("r2e://gone"),
    )
    .await;
    assert!(
        subscribed.message().get("error").is_none(),
        "{}",
        subscribed.raw_body
    );
    updates.notify("r2e://gone");
    r2e_core::rt::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let frame = body.frame().await.expect("SSE stream ended").unwrap();
            if let Some(data) = frame.data_ref() {
                let data = String::from_utf8_lossy(data);
                if data.contains("notifications/resources/updated") && data.contains("r2e://gone") {
                    return;
                }
            }
        }
    })
    .await
    .expect("a fresh subscription must deliver updates for the replacement");
}
