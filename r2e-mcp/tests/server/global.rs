//! The global dynamic member registry (`McpRegistry`): members every
//! session serves, added and removed after boot; collision rules against
//! the catalog and session-private members; atomic `edit`; `list_changed`
//! fan-out; an endpoint with no service at all.

use http_body_util::BodyExt;
use r2e_core::http::{Body, Request, Router, StatusCode};
use r2e_core::prelude::*;
use r2e_core::AppBuilder;
use r2e_mcp::{
    AppBuilderMcpExt, DynamicPrompt, DynamicResource, DynamicTool, McpMemberError, McpRegistry,
    McpServer, McpSessions, PromptCall, ResourceCall, ToolCall, ToolRoute,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use tower::ServiceExt;

use crate::support::{self, initialize, tools_call, tools_list};

// ── Services ───────────────────────────────────────────────────────────────

#[derive(Deserialize, JsonSchema, ObjectParams)]
pub struct EchoIn {
    pub text: String,
}

#[controller]
pub struct Base;

#[mcp_routes]
impl Base {
    /// Always visible.
    #[tool]
    async fn ping(&self) -> &'static str {
        "pong"
    }

    /// Another catalog tool (pagination filler).
    #[tool]
    async fn pong(&self) -> &'static str {
        "ping"
    }

    /// A catalog resource template.
    #[resource(uri = "file://{path}")]
    async fn file(&self, call: ResourceCall) -> String {
        call.variables["path"].clone()
    }
}

#[controller]
pub struct Hidden;

#[mcp_routes(group = "lab", opt_in)]
impl Hidden {
    /// Hidden by default — still owns its name.
    #[tool]
    async fn secret(&self) -> &'static str {
        "shh"
    }
}

struct App {
    router: Router,
    registry: McpRegistry,
    sessions: McpSessions,
}

async fn app_with(plugin: McpServer) -> App {
    let app = AppBuilder::new()
        .plugin(plugin)
        .build_state()
        .await
        .register_mcp_service::<Base>()
        .register_mcp_service::<Hidden>();
    let registry = app
        .bean_context()
        .try_get::<McpRegistry>()
        .expect("McpServer provides McpRegistry");
    let sessions = app.bean_context().try_get::<McpSessions>().unwrap();
    App {
        router: app.build(),
        registry,
        sessions,
    }
}

async fn app() -> App {
    app_with(McpServer::new()).await
}

fn text_of(msg: &Value) -> &str {
    msg["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no text content in {msg}"))
}

fn echo_tool(name: &str, reply: &'static str) -> ToolRoute {
    echo_builder(name, reply)
        .handler(move |p: EchoIn, _call: ToolCall| async move { format!("{reply}:{}", p.text) })
}

fn echo_builder(name: &str, reply: &'static str) -> DynamicTool {
    DynamicTool::new(name.to_owned()).description(reply)
}

async fn list(router: &Router, session: &str, method: &str, params: Value) -> Value {
    support::post(
        router,
        "/mcp",
        Some(session),
        &json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": params }),
    )
    .await
    .result()
    .clone()
}

/// Open the session's SSE stream (GET) and collect frames until `done`.
async fn sse_until(router: &Router, session: &str, done: impl Fn(&str) -> bool) -> Body {
    let stream = router
        .clone()
        .oneshot(
            Request::get("/mcp")
                .header("host", "localhost")
                .header("accept", "text/event-stream")
                .header("mcp-session-id", session)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stream.status(), StatusCode::OK);
    let _ = done;
    stream.into_body()
}

async fn collect_until(body: &mut Body, done: impl Fn(&str) -> bool) -> String {
    r2e_core::rt::timeout(std::time::Duration::from_secs(2), async {
        let mut seen = String::new();
        while !done(&seen) {
            let frame = body.frame().await.expect("SSE stream ended").unwrap();
            if let Some(data) = frame.data_ref() {
                seen.push_str(&String::from_utf8_lossy(data));
            }
        }
        seen
    })
    .await
    .expect("list_changed notifications timed out")
}

// ── Visibility across sessions ─────────────────────────────────────────────

#[r2e_core::test]
async fn global_tool_is_served_to_every_session() {
    let App {
        router, registry, ..
    } = app().await;
    let a = initialize(&router, "/mcp").await;
    let b = initialize(&router, "/mcp").await;
    assert!(registry.is_empty());

    registry.add_tool(echo_tool("shout", "loud")).unwrap();
    assert!(registry.has_tool("shout"));
    assert_eq!(registry.tool_names(), vec!["shout".to_string()]);

    for session in [&a, &b] {
        let listed = tools_list(&router, "/mcp", session).await;
        let shout = support::tool(&listed, "shout");
        assert_eq!(
            shout["inputSchema"]["properties"]["text"]["type"], "string",
            "{shout}"
        );
        let called = tools_call(&router, "/mcp", session, "shout", json!({ "text": "hi" })).await;
        assert_eq!(text_of(&called), "loud:hi", "{called}");
    }
    // A session opened AFTER the add sees it too.
    let c = initialize(&router, "/mcp").await;
    assert!(support::tool_names(&tools_list(&router, "/mcp", &c).await).contains(&"shout".into()));

    assert!(registry.remove_tool("shout"));
    assert!(!registry.remove_tool("shout"));
    assert!(!registry.has_tool("shout"));
    for session in [&a, &b, &c] {
        let gone = tools_call(&router, "/mcp", session, "shout", json!({ "text": "hi" })).await;
        assert_eq!(gone["error"]["code"], -32601, "{gone}");
        assert!(
            !support::tool_names(&tools_list(&router, "/mcp", session).await)
                .contains(&"shout".into())
        );
    }
}

#[r2e_core::test]
async fn upsert_replaces_a_registry_tool_in_place() {
    let App {
        router, registry, ..
    } = app().await;
    let session = initialize(&router, "/mcp").await;
    assert!(!registry.upsert_tool(echo_tool("v", "one")).unwrap());
    assert!(registry.upsert_tool(echo_tool("v", "two")).unwrap());
    let called = tools_call(&router, "/mcp", &session, "v", json!({ "text": "x" })).await;
    assert_eq!(text_of(&called), "two:x", "{called}");
    let names = support::tool_names(&tools_list(&router, "/mcp", &session).await);
    assert_eq!(names.iter().filter(|n| *n == "v").count(), 1, "{names:?}");
    // Upserting over a catalog name is still a clash.
    assert!(matches!(
        registry.upsert_tool(echo_tool("ping", "x")),
        Err(McpMemberError::Duplicate { kind: "tool", .. })
    ));
}

// ── Collision rules ────────────────────────────────────────────────────────

#[r2e_core::test]
async fn catalog_keys_are_refused_even_when_hidden() {
    let App { registry, .. } = app().await;
    for name in ["ping", "secret"] {
        let err = registry.add_tool(echo_tool(name, "x")).unwrap_err();
        assert!(
            matches!(&err, McpMemberError::Duplicate { kind: "tool", key } if key == name),
            "{err:?}"
        );
    }
    // Same shape as the catalog's `file://{path}` template.
    let err = registry
        .add_resource(
            DynamicResource::new("file://{name}", "other")
                .handler(|_call: ResourceCall| async { "x" }),
        )
        .unwrap_err();
    assert!(
        matches!(
            &err,
            McpMemberError::Duplicate {
                kind: "resource template",
                ..
            }
        ),
        "{err:?}"
    );
    // A registry namesake is a clash too.
    registry.add_tool(echo_tool("mine", "x")).unwrap();
    let err = registry.add_tool(echo_tool("mine", "y")).unwrap_err();
    assert!(matches!(err, McpMemberError::Duplicate { .. }), "{err:?}");
}

#[r2e_core::test]
async fn session_private_member_shadows_the_global_one() {
    let App {
        router,
        registry,
        sessions,
    } = app().await;
    let a = initialize(&router, "/mcp").await;
    let b = initialize(&router, "/mcp").await;
    registry.add_tool(echo_tool("echo", "global")).unwrap();
    // No cross-check: the session may take the same name...
    sessions
        .get(&a)
        .unwrap()
        .add_tool(echo_tool("echo", "private"))
        .unwrap();
    // ...and wins for that session only.
    let from_a = tools_call(&router, "/mcp", &a, "echo", json!({ "text": "1" })).await;
    assert_eq!(text_of(&from_a), "private:1", "{from_a}");
    let from_b = tools_call(&router, "/mcp", &b, "echo", json!({ "text": "1" })).await;
    assert_eq!(text_of(&from_b), "global:1", "{from_b}");

    // The list carries ONE `echo` entry per session: the session's own.
    let list_a = tools_list(&router, "/mcp", &a).await;
    let names = support::tool_names(&list_a);
    assert_eq!(
        names.iter().filter(|n| *n == "echo").count(),
        1,
        "{names:?}"
    );
    assert_eq!(support::tool(&list_a, "echo")["description"], "private");
    assert_eq!(
        support::tool(&tools_list(&router, "/mcp", &b).await, "echo")["description"],
        "global"
    );

    // Dropping the private one uncovers the global one.
    assert!(sessions.get(&a).unwrap().remove_tool("echo").unwrap());
    let from_a = tools_call(&router, "/mcp", &a, "echo", json!({ "text": "2" })).await;
    assert_eq!(text_of(&from_a), "global:2", "{from_a}");
    assert!(sessions.get(&a).unwrap().has_tool("echo"));
}

// ── Resources and prompts ──────────────────────────────────────────────────

#[r2e_core::test]
async fn global_resources_templates_and_prompts_are_served() {
    let App {
        router, registry, ..
    } = app().await;
    let session = initialize(&router, "/mcp").await;
    registry
        .edit(|edit| {
            edit.add_resource(
                DynamicResource::new("ui://widget", "widget")
                    .mime_type("text/html")
                    .handler(|_call: ResourceCall| async { "<b>widget</b>" }),
            )?;
            edit.add_resource(
                DynamicResource::new("ui://{app}/view", "view")
                    .handler(|call: ResourceCall| async move { call.variables["app"].clone() }),
            )?;
            edit.add_prompt(
                DynamicPrompt::new("summarize")
                    .description("Summarize the text")
                    .handler(
                        |p: EchoIn, _call: PromptCall| async move { format!("sum:{}", p.text) },
                    ),
            )
        })
        .unwrap();
    assert!(registry.has_resource("ui://widget"));
    assert!(registry.has_resource("ui://{app}/view"));
    assert!(registry.has_prompt("summarize"));

    let resources = support::resources_list(&router, "/mcp", &session).await;
    assert_eq!(
        support::resource(&resources, "ui://widget")["mimeType"],
        "text/html"
    );
    let read = support::resources_read(&router, "/mcp", &session, "ui://widget").await;
    assert_eq!(
        read["result"]["contents"][0]["text"], "<b>widget</b>",
        "{read}"
    );

    let templates = list(&router, &session, "resources/templates/list", json!({})).await;
    let uris: Vec<&str> = templates["resourceTemplates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["uriTemplate"].as_str().unwrap())
        .collect();
    assert!(uris.contains(&"ui://{app}/view"), "{uris:?}");
    assert!(uris.contains(&"file://{path}"), "{uris:?}");
    let read = support::resources_read(&router, "/mcp", &session, "ui://plexus/view").await;
    assert_eq!(read["result"]["contents"][0]["text"], "plexus", "{read}");

    let prompts = support::prompts_list(&router, "/mcp", &session).await;
    assert_eq!(
        support::prompt(&prompts, "summarize")["arguments"][0]["name"],
        "text"
    );
    let got = support::prompts_get(
        &router,
        "/mcp",
        &session,
        "summarize",
        json!({ "text": "t" }),
    )
    .await;
    assert_eq!(
        got["result"]["messages"][0]["content"]["text"], "sum:t",
        "{got}"
    );

    assert!(registry.remove_resource("ui://{app}/view"));
    assert!(registry.remove_prompt("summarize"));
    let read = support::resources_read(&router, "/mcp", &session, "ui://plexus/view").await;
    assert_eq!(read["error"]["code"], -32002, "{read}");
    let got = support::prompts_get(
        &router,
        "/mcp",
        &session,
        "summarize",
        json!({ "text": "t" }),
    )
    .await;
    assert!(got.get("error").is_some(), "{got}");
}

// ── Atomic edits + notifications ───────────────────────────────────────────

#[r2e_core::test]
async fn edit_is_atomic_and_notifies_each_family_once() {
    let App {
        router, registry, ..
    } = app().await;
    let session = initialize(&router, "/mcp").await;
    let mut stream = sse_until(&router, &session, |_| true).await;

    // A failing step rolls the whole batch back.
    let err = registry
        .edit(|edit| {
            edit.add_tool(echo_tool("one", "1"))?;
            edit.add_tool(echo_tool("two", "2"))?;
            edit.add_tool(echo_tool("ping", "clash"))
        })
        .unwrap_err();
    assert!(matches!(err, McpMemberError::Duplicate { .. }), "{err:?}");
    assert!(registry.is_empty());
    assert!(
        !support::tool_names(&tools_list(&router, "/mcp", &session).await).contains(&"one".into())
    );

    // A whole batch lands at once: tools + prompts changed, resources not.
    registry
        .edit(|edit| {
            edit.add_tool(echo_tool("one", "1"))?;
            edit.add_tool(echo_tool("two", "2"))?;
            edit.add_prompt(
                DynamicPrompt::new("p").handler(|p: EchoIn, _c: PromptCall| async move { p.text }),
            )
        })
        .unwrap();
    let seen = collect_until(&mut stream, |seen| {
        seen.contains("notifications/tools/list_changed")
            && seen.contains("notifications/prompts/list_changed")
    })
    .await;
    assert_eq!(
        seen.matches("notifications/tools/list_changed").count(),
        1,
        "{seen}"
    );
    assert!(
        !seen.contains("notifications/resources/list_changed"),
        "{seen}"
    );

    // A no-op edit (remove of an absent member) notifies nothing; the next
    // real change is the only frame that follows.
    assert!(!registry.remove_tool("absent"));
    assert!(registry.remove_tool("two"));
    let seen = collect_until(&mut stream, |seen| {
        seen.contains("notifications/tools/list_changed")
    })
    .await;
    assert_eq!(seen.matches("list_changed").count(), 1, "{seen}");
}

// ── Boot-time shapes ───────────────────────────────────────────────────────

#[r2e_core::test]
async fn endpoint_is_mounted_without_any_service() {
    let app = AppBuilder::new()
        .plugin(McpServer::new())
        .build_state()
        .await;
    let registry = app.bean_context().try_get::<McpRegistry>().unwrap();
    let router = app.build();
    let session = initialize(&router, "/mcp").await;
    assert!(support::tool_names(&tools_list(&router, "/mcp", &session).await).is_empty());
    registry.add_tool(echo_tool("only", "o")).unwrap();
    let called = tools_call(&router, "/mcp", &session, "only", json!({ "text": "k" })).await;
    assert_eq!(text_of(&called), "o:k", "{called}");
}

#[r2e_core::test]
async fn members_added_before_the_mount_are_served() {
    let app = AppBuilder::new()
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_mcp_service::<Base>();
    let registry = app.bean_context().try_get::<McpRegistry>().unwrap();
    // Before `build()` mounts the endpoint — e.g. from a `#[post_construct]`.
    registry.add_tool(echo_tool("early", "e")).unwrap();
    let router = app.build();
    let session = initialize(&router, "/mcp").await;
    let called = tools_call(&router, "/mcp", &session, "early", json!({ "text": "k" })).await;
    assert_eq!(text_of(&called), "e:k", "{called}");
}

#[r2e_core::test]
#[should_panic(expected = "added before boot is invalid")]
async fn a_pre_mount_catalog_clash_aborts_startup() {
    let app = AppBuilder::new()
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_mcp_service::<Base>();
    let registry = app.bean_context().try_get::<McpRegistry>().unwrap();
    registry.add_tool(echo_tool("ping", "clash")).unwrap();
    let _ = app.build();
}

#[r2e_core::test]
async fn scopes_without_auth_are_refused() {
    let App { registry, .. } = app().await;
    let err = registry
        .add_tool(
            echo_builder("scoped", "s")
                .scopes(&["tools:write"])
                .handler(|p: EchoIn, _call: ToolCall| async move { p.text }),
        )
        .unwrap_err();
    assert!(
        matches!(err, McpMemberError::ScopesWithoutAuth { kind: "tool", .. }),
        "{err:?}"
    );
}

// ── Pagination over the merged list ────────────────────────────────────────

#[r2e_core::test]
async fn pages_walk_catalog_then_global_members_without_gaps() {
    let App {
        router, registry, ..
    } = app_with(McpServer::new().with_page_size(2)).await;
    let session = initialize(&router, "/mcp").await;
    registry
        .edit(|edit| {
            edit.add_tool(echo_tool("g1", "1"))?;
            edit.add_tool(echo_tool("g2", "2"))?;
            edit.add_tool(echo_tool("g3", "3"))
        })
        .unwrap();
    let mut names = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let params = match &cursor {
            Some(c) => json!({ "cursor": c }),
            None => json!({}),
        };
        let page = list(&router, &session, "tools/list", params).await;
        let got = support::tool_names(&page);
        assert!(got.len() <= 2, "{got:?}");
        names.extend(got);
        match page["nextCursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => break,
        }
    }
    names.sort();
    assert_eq!(names, vec!["g1", "g2", "g3", "ping", "pong"]);
}
