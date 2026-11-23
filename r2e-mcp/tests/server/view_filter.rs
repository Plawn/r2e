//! Groups on dynamic members and the per-request view filter (#1446).
//!
//! * a `Dynamic*` member declares a group (`.group(..)` / `.opt_in_group(..)`):
//!   registry members are gated per session at request time, session-private
//!   ones at the view rebuild; switching such a group notifies `list_changed`;
//! * `McpServer::view_filter::<T>()`: a bean asked about every member, on
//!   every list and every lookup — a hidden member answers like an unknown
//!   one, over sessions, stateless serving and the loopback;
//! * `McpViewFilter::listed` (#1467): a member that exists may be left out
//!   of the lists and still answer by name; a hidden one stays hidden.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use http_body_util::BodyExt;
use r2e_core::http::{Body, Request, Routes, StatusCode};
use r2e_core::prelude::*;
use r2e_core::AppBuilder;
use r2e_mcp::{
    AppBuilderMcpExt, DynamicPrompt, DynamicResource, DynamicTool, McpMember, McpMemberError,
    McpRegistry, McpServer, McpSessions, McpView, McpViewFilter, PromptCall, ResourceCall,
    SessionToolset, ToolCall, ToolRoute,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use tower::ServiceExt;

use crate::support::{self, initialize};

// ── Fixtures ───────────────────────────────────────────────────────────────

#[derive(Deserialize, JsonSchema, ObjectParams)]
pub struct EchoIn {
    pub text: String,
}

#[controller]
pub struct Open;

#[routes]
impl Open {
    /// Always there.
    #[tool]
    async fn ping(&self) -> &'static str {
        "pong"
    }
}

#[controller]
pub struct Lab;

#[routes(group = "lab", opt_in)]
impl Lab {
    /// Off by default.
    #[tool]
    async fn secret(&self) -> &'static str {
        "shh"
    }
}

#[controller]
pub struct Staff;

#[routes(group = "staff")]
impl Staff {
    /// A staff tool.
    #[tool]
    async fn audit(&self) -> &'static str {
        "audited"
    }

    /// A staff resource.
    #[resource(uri = "staff://handbook")]
    async fn handbook(&self) -> &'static str {
        "rules"
    }

    /// A staff resource template.
    #[resource(uri = "staff://people/{name}")]
    async fn person(&self, call: ResourceCall) -> String {
        call.variables["name"].clone()
    }

    /// A staff prompt.
    #[prompt]
    async fn brief(&self) -> &'static str {
        "be brief"
    }
}

/// What an upstream layer (or the loopback caller) knows about the request.
#[derive(Clone, Debug, PartialEq)]
struct Workspace(&'static str);

/// The application's filter:
///
/// * the `staff` group exists for `x-tier: staff` requests only;
/// * a member tagged `_meta.class = "internal"` exists only for a request
///   carrying a [`Workspace`] extension;
/// * `needs_lab` exists only while the session has the `lab` group on;
/// * anything named in `blocked` is gone;
/// * anything named in `unlisted` exists but is left out of the lists.
#[derive(Clone, Default)]
struct Gate {
    blocked: Arc<Mutex<HashSet<String>>>,
    unlisted: Arc<Mutex<HashSet<String>>>,
    seen_sessions: Arc<Mutex<HashSet<String>>>,
}

impl McpViewFilter for Gate {
    fn visible(&self, member: McpMember<'_>, view: &McpView<'_>) -> bool {
        if let Some(id) = view.session_id() {
            self.seen_sessions.lock().unwrap().insert(id.to_string());
        }
        if self.blocked.lock().unwrap().contains(member.name()) {
            return false;
        }
        if member.group() == Some("staff") && view.header("x-tier") != Some("staff") {
            return false;
        }
        let internal = member
            .meta()
            .and_then(|meta| meta.get("class"))
            .is_some_and(|class| class == "internal");
        if internal && view.extension::<Workspace>().is_none() {
            return false;
        }
        if member.name() == "needs_lab" {
            return member.is_tool() && view.is_group_enabled("lab");
        }
        true
    }

    fn listed(&self, member: McpMember<'_>, _view: &McpView<'_>) -> bool {
        !self.unlisted.lock().unwrap().contains(member.name())
    }
}

impl Gate {
    fn unlist(&self, names: &[&str]) {
        let mut unlisted = self.unlisted.lock().unwrap();
        unlisted.extend(names.iter().map(|name| name.to_string()));
    }
}

struct App {
    router: Routes,
    registry: McpRegistry,
    sessions: McpSessions,
}

/// The builder's type changes with what it provides: a macro, not a fn.
macro_rules! boot {
    ($builder:expr, $plugin:expr) => {{
        let app = $builder
            .plugin($plugin)
            .build_state()
            .await
            .register_mcp_service::<Open>()
            .register_mcp_service::<Lab>()
            .register_mcp_service::<Staff>();
        let registry = app.bean_context().try_get::<McpRegistry>().unwrap();
        let sessions = app.bean_context().try_get::<McpSessions>().unwrap();
        App {
            router: app.build(),
            registry,
            sessions,
        }
    }};
}

async fn plain() -> App {
    boot!(AppBuilder::new(), McpServer::new())
}

async fn gated(gate: Gate) -> App {
    boot!(
        AppBuilder::new().provide(gate),
        McpServer::new().view_filter::<Gate>()
    )
}

fn echo(name: &'static str) -> DynamicTool {
    DynamicTool::new(name).description(name)
}

fn echoing(tool: DynamicTool, reply: &'static str) -> ToolRoute {
    tool.handler(move |p: EchoIn, _call: ToolCall| async move { format!("{reply}:{}", p.text) })
}

fn internal() -> rmcp::model::MetaObject {
    serde_json::from_value(json!({ "class": "internal" })).unwrap()
}

/// One JSON-RPC request; the whole message back (`result` or `error`).
async fn rpc(
    router: &Routes,
    session: Option<&str>,
    headers: &[(&str, &str)],
    method: &str,
    params: Value,
) -> Value {
    let response = support::post_with_headers(
        router,
        "/mcp",
        session,
        headers,
        &json!({ "jsonrpc": "2.0", "id": 9, "method": method, "params": params }),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);
    response.message().clone()
}

/// The names (or URIs) a list method returns.
async fn listed(
    router: &Routes,
    session: Option<&str>,
    headers: &[(&str, &str)],
    family: &str,
) -> Vec<String> {
    let (method, array, key) = match family {
        "tools" => ("tools/list", "tools", "name"),
        "resources" => ("resources/list", "resources", "uri"),
        "templates" => (
            "resources/templates/list",
            "resourceTemplates",
            "uriTemplate",
        ),
        "prompts" => ("prompts/list", "prompts", "name"),
        other => panic!("unknown family {other}"),
    };
    let message = rpc(router, session, headers, method, json!({})).await;
    let mut names: Vec<String> = message["result"][array]
        .as_array()
        .unwrap_or_else(|| panic!("no `{array}` in {message}"))
        .iter()
        .map(|item| item[key].as_str().unwrap().to_owned())
        .collect();
    names.sort();
    names
}

async fn call(router: &Routes, session: Option<&str>, headers: &[(&str, &str)], name: &str) -> Value {
    rpc(
        router,
        session,
        headers,
        "tools/call",
        json!({ "name": name, "arguments": { "text": "x" } }),
    )
    .await
}

fn text_of(message: &Value) -> &str {
    message["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no text content in {message}"))
}

async fn open_stream(router: &Routes, session: &str) -> Body {
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
    .expect("list_changed notification timed out")
}

// ── Groups on dynamic members ──────────────────────────────────────────────

#[r2e_core::test]
async fn a_registry_group_is_switched_per_session() {
    let App {
        router,
        registry,
        sessions,
    } = plain().await;
    registry
        .edit(|edit| {
            edit.add_tool(echoing(echo("beta_tool").opt_in_group("beta"), "beta"))?;
            edit.add_resource(
                DynamicResource::new("beta://notes", "notes")
                    .opt_in_group("beta")
                    .handler(|_call: ResourceCall| async { "notes" }),
            )?;
            edit.add_resource(
                DynamicResource::new("beta://{doc}", "doc")
                    .opt_in_group("beta")
                    .handler(|call: ResourceCall| async move { call.variables["doc"].clone() }),
            )?;
            edit.add_prompt(
                DynamicPrompt::new("extra")
                    .group("extras")
                    .handler(|_p: EchoIn, _call: PromptCall| async { "extra" }),
            )
        })
        .unwrap();
    let a = initialize(&router, "/mcp").await;
    let b = initialize(&router, "/mcp").await;
    let (sa, sb) = (Some(a.as_str()), Some(b.as_str()));

    // Opt-in: hidden and unknown until the session enables it; the plain
    // group is on.
    assert_eq!(listed(&router, sa, &[], "tools").await, ["audit", "ping"]);
    assert_eq!(listed(&router, sa, &[], "prompts").await, ["brief", "extra"]);
    assert_eq!(call(&router, sa, &[], "beta_tool").await["error"]["code"], -32601);
    let read = rpc(&router, sa, &[], "resources/read", json!({ "uri": "beta://notes" })).await;
    assert_eq!(read["error"]["code"], -32002, "{read}");
    let read = rpc(&router, sa, &[], "resources/read", json!({ "uri": "beta://x" })).await;
    assert_eq!(read["error"]["code"], -32002, "{read}");

    let live = sessions.get(&a).unwrap();
    assert!(!live.is_group_enabled("beta"));
    assert!(live.is_group_enabled("extras"));
    assert!(!live.has_tool("beta_tool"));
    assert_eq!(live.groups(), ["lab", "staff", "beta", "extras"]);

    live.enable_group("beta").unwrap();
    assert!(live.is_group_enabled("beta"));
    assert!(live.has_tool("beta_tool"));
    assert_eq!(
        listed(&router, sa, &[], "tools").await,
        ["audit", "beta_tool", "ping"]
    );
    assert_eq!(text_of(&call(&router, sa, &[], "beta_tool").await), "beta:x");
    assert!(listed(&router, sa, &[], "resources")
        .await
        .contains(&"beta://notes".to_string()));
    assert!(listed(&router, sa, &[], "templates")
        .await
        .contains(&"beta://{doc}".to_string()));
    let read = rpc(&router, sa, &[], "resources/read", json!({ "uri": "beta://x" })).await;
    assert_eq!(read["result"]["contents"][0]["text"], "x", "{read}");

    // The other session is untouched.
    assert_eq!(listed(&router, sb, &[], "tools").await, ["audit", "ping"]);
    assert_eq!(call(&router, sb, &[], "beta_tool").await["error"]["code"], -32601);

    // A default-on dynamic group can be switched off.
    live.disable_group("extras").unwrap();
    assert_eq!(listed(&router, sa, &[], "prompts").await, ["brief"]);
    let got = rpc(
        &router,
        sa,
        &[],
        "prompts/get",
        json!({ "name": "extra", "arguments": { "text": "x" } }),
    )
    .await;
    assert_eq!(got["error"]["code"], -32602, "{got}");
    assert_eq!(listed(&router, sb, &[], "prompts").await, ["brief", "extra"]);

    live.disable_group("beta").unwrap();
    assert_eq!(call(&router, sa, &[], "beta_tool").await["error"]["code"], -32601);
}

#[r2e_core::test]
async fn a_session_private_group_is_switched_by_its_session() {
    let App {
        router, sessions, ..
    } = plain().await;
    let a = initialize(&router, "/mcp").await;
    let sa = Some(a.as_str());
    let live = sessions.get(&a).unwrap();

    live.add_tool(echoing(echo("mine").opt_in_group("own"), "mine"))
        .unwrap();
    assert!(!live.has_tool("mine"));
    assert_eq!(call(&router, sa, &[], "mine").await["error"]["code"], -32601);
    assert!(!listed(&router, sa, &[], "tools").await.contains(&"mine".to_string()));

    live.enable_group("own").unwrap();
    assert_eq!(text_of(&call(&router, sa, &[], "mine").await), "mine:x");
    assert!(listed(&router, sa, &[], "tools").await.contains(&"mine".to_string()));

    // One batch may add a grouped member and switch its group.
    live.apply(
        SessionToolset::new()
            .tool(echoing(echo("other").opt_in_group("more"), "other"))
            .enable("more"),
    )
    .unwrap();
    assert_eq!(text_of(&call(&router, sa, &[], "other").await), "other:x");
}

#[r2e_core::test]
async fn group_names_and_opt_in_are_checked() {
    let App {
        router,
        registry,
        sessions,
    } = plain().await;
    let a = initialize(&router, "/mcp").await;
    let live = sessions.get(&a).unwrap();

    // No member declares it: not a group.
    assert!(matches!(
        live.enable_group("nope"),
        Err(McpMemberError::UnknownGroup(group)) if group == "nope"
    ));
    // The catalog's `lab` is opt-in: a dynamic member cannot join it as a
    // default-on group…
    assert!(matches!(
        registry.add_tool(echoing(echo("x").group("lab"), "x")),
        Err(McpMemberError::GroupMismatch(group)) if group == "lab"
    ));
    // …but joins it with the same declaration, and follows its switch.
    registry
        .add_tool(echoing(echo("lab_extra").opt_in_group("lab"), "lab"))
        .unwrap();
    assert!(!live.has_tool("lab_extra"));
    live.enable_group("lab").unwrap();
    assert!(live.has_tool("lab_extra"));
    assert!(live.has_tool("secret"));

    // Two dynamic members of one group agree too — within the registry,
    // and between a session and the registry.
    registry
        .add_tool(echoing(echo("g1").opt_in_group("g"), "g1"))
        .unwrap();
    assert!(matches!(
        registry.add_tool(echoing(echo("g2").group("g"), "g2")),
        Err(McpMemberError::GroupMismatch(_))
    ));
    assert!(matches!(
        live.add_tool(echoing(echo("g3").group("g"), "g3")),
        Err(McpMemberError::GroupMismatch(_))
    ));
    assert!(!registry.has_tool("g2"));
}

#[r2e_core::test]
async fn switching_a_registry_group_notifies_list_changed() {
    let App {
        router,
        registry,
        sessions,
    } = plain().await;
    registry
        .add_tool(echoing(echo("beta_tool").opt_in_group("beta"), "beta"))
        .unwrap();
    let a = initialize(&router, "/mcp").await;
    let mut stream = open_stream(&router, &a).await;
    let live = sessions.get(&a).unwrap();

    live.enable_group("beta").unwrap();
    let seen = collect_until(&mut stream, |seen| {
        seen.contains("notifications/tools/list_changed")
    })
    .await;
    assert!(!seen.contains("notifications/prompts/list_changed"), "{seen}");

    // Already on: nothing to say; the explicit nudge is the next frame.
    live.enable_group("beta").unwrap();
    live.notify_list_changed();
    let seen = collect_until(&mut stream, |seen| {
        seen.contains("notifications/prompts/list_changed")
    })
    .await;
    assert_eq!(
        seen.matches("notifications/tools/list_changed").count(),
        1,
        "{seen}"
    );
}

// ── The view filter ────────────────────────────────────────────────────────

#[r2e_core::test]
async fn the_filter_hides_members_from_lists_and_lookups() {
    let gate = Gate::default();
    let App { router, .. } = gated(gate.clone()).await;
    let a = initialize(&router, "/mcp").await;
    let sa = Some(a.as_str());
    let staff: &[(&str, &str)] = &[("x-tier", "staff")];

    // Not staff: the group's members do not exist.
    assert_eq!(listed(&router, sa, &[], "tools").await, ["ping"]);
    assert!(listed(&router, sa, &[], "resources").await.is_empty());
    assert!(listed(&router, sa, &[], "templates").await.is_empty());
    assert!(listed(&router, sa, &[], "prompts").await.is_empty());
    assert_eq!(call(&router, sa, &[], "audit").await["error"]["code"], -32601);
    for uri in ["staff://handbook", "staff://people/ada"] {
        let read = rpc(&router, sa, &[], "resources/read", json!({ "uri": uri })).await;
        assert_eq!(read["error"]["code"], -32002, "{read}");
        let sub = rpc(&router, sa, &[], "resources/subscribe", json!({ "uri": uri })).await;
        assert_eq!(sub["error"]["code"], -32002, "{sub}");
    }
    let got = rpc(&router, sa, &[], "prompts/get", json!({ "name": "brief" })).await;
    assert_eq!(got["error"]["code"], -32602, "{got}");

    // Same session, a staff request: they do.
    assert_eq!(listed(&router, sa, staff, "tools").await, ["audit", "ping"]);
    assert_eq!(
        listed(&router, sa, staff, "resources").await,
        ["staff://handbook"]
    );
    assert_eq!(
        listed(&router, sa, staff, "templates").await,
        ["staff://people/{name}"]
    );
    assert_eq!(listed(&router, sa, staff, "prompts").await, ["brief"]);
    assert_eq!(text_of(&call(&router, sa, staff, "audit").await), "audited");
    let read = rpc(
        &router,
        sa,
        staff,
        "resources/read",
        json!({ "uri": "staff://people/ada" }),
    )
    .await;
    assert_eq!(read["result"]["contents"][0]["text"], "ada", "{read}");
    let got = rpc(&router, sa, staff, "prompts/get", json!({ "name": "brief" })).await;
    assert!(got["result"]["messages"].is_array(), "{got}");

    // The filter saw the session it was asked for.
    assert!(gate.seen_sessions.lock().unwrap().contains(&a));

    // State the bean holds decides too, and may change at any time.
    gate.blocked.lock().unwrap().insert("ping".into());
    assert!(listed(&router, sa, &[], "tools").await.is_empty());
    assert_eq!(call(&router, sa, &[], "ping").await["error"]["code"], -32601);
    gate.blocked.lock().unwrap().clear();
    assert_eq!(text_of(&call(&router, sa, &[], "ping").await), "pong");
}

#[r2e_core::test]
async fn the_filter_covers_dynamic_members_and_a_hidden_one_does_not_shadow() {
    let gate = Gate::default();
    let App {
        router,
        registry,
        sessions,
    } = gated(gate.clone()).await;
    let a = initialize(&router, "/mcp").await;
    let sa = Some(a.as_str());
    let live = sessions.get(&a).unwrap();

    // The registry's `dup`, and a session-private `dup` the filter hides
    // (no `Workspace` on an HTTP request): the registry's is the one served.
    registry.add_tool(echoing(echo("dup"), "global")).unwrap();
    live.add_tool(echoing(echo("dup").meta(internal()), "private"))
        .unwrap();
    assert_eq!(text_of(&call(&router, sa, &[], "dup").await), "global:x");
    let tools = listed(&router, sa, &[], "tools").await;
    assert_eq!(tools, ["dup", "ping"]);

    // A registry member the filter hides is gone for everyone it hides it
    // from.
    registry
        .add_tool(echoing(echo("ops").meta(internal()), "ops"))
        .unwrap();
    assert_eq!(call(&router, sa, &[], "ops").await["error"]["code"], -32601);
    assert!(!listed(&router, sa, &[], "tools").await.contains(&"ops".to_string()));

    // The filter reads the session's groups.
    registry.add_tool(echoing(echo("needs_lab"), "lab")).unwrap();
    assert_eq!(call(&router, sa, &[], "needs_lab").await["error"]["code"], -32601);
    live.enable_group("lab").unwrap();
    assert_eq!(text_of(&call(&router, sa, &[], "needs_lab").await), "lab:x");
    assert_eq!(
        listed(&router, sa, &[], "tools").await,
        ["dup", "needs_lab", "ping", "secret"]
    );
}

#[r2e_core::test]
async fn the_filter_runs_on_stateless_requests() {
    let App { router, .. } = boot!(
        AppBuilder::new().provide(Gate::default()),
        McpServer::new()
            .stateless(true)
            .json_response(true)
            .view_filter::<Gate>()
    );
    assert_eq!(listed(&router, None, &[], "tools").await, ["ping"]);
    assert_eq!(
        listed(&router, None, &[("x-tier", "staff")], "tools").await,
        ["audit", "ping"]
    );
    assert_eq!(call(&router, None, &[], "audit").await["error"]["code"], -32601);
    assert_eq!(
        text_of(&call(&router, None, &[("x-tier", "staff")], "audit").await),
        "audited"
    );
}

#[cfg(feature = "loopback")]
#[r2e_core::test]
async fn the_filter_reads_loopback_extensions() {
    use r2e_mcp::McpLoopback;
    use rmcp::model::CallToolRequestParams;

    let app = AppBuilder::new()
        .provide(Gate::default())
        .plugin(McpServer::new().view_filter::<Gate>())
        .build_state()
        .await
        .register_mcp_service::<Open>();
    let registry = app.bean_context().try_get::<McpRegistry>().unwrap();
    let loopback = app.bean_context().try_get::<McpLoopback>().unwrap();
    let _router = app.build();
    registry
        .add_tool(echoing(echo("ops").meta(internal()), "ops"))
        .unwrap();

    let names = |tools: Vec<rmcp::model::Tool>| {
        let mut names: Vec<String> = tools.into_iter().map(|t| t.name.to_string()).collect();
        names.sort();
        names
    };
    let outside = loopback.session("plain").connect().await.unwrap();
    assert_eq!(names(outside.list_all_tools().await.unwrap()), ["ping"]);
    assert!(outside
        .call_tool(CallToolRequestParams::new("ops"))
        .await
        .is_err());

    let inside = loopback
        .session("ws")
        .extension(Workspace("acme"))
        .connect()
        .await
        .unwrap();
    assert_eq!(names(inside.list_all_tools().await.unwrap()), ["ops", "ping"]);
}

// ── Listed vs. reachable by name (#1467) ───────────────────────────────────

#[r2e_core::test]
async fn an_unlisted_member_is_left_out_of_lists_and_answers_by_name() {
    let gate = Gate::default();
    let App {
        router, registry, ..
    } = gated(gate.clone()).await;
    registry
        .edit(|edit| {
            edit.add_tool(echoing(echo("tail"), "tail"))?;
            edit.add_resource(
                DynamicResource::new("tail://notes", "notes")
                    .handler(|_call: ResourceCall| async { "notes" }),
            )?;
            edit.add_prompt(
                DynamicPrompt::new("tail_prompt")
                    .handler(|_p: EchoIn, _call: PromptCall| async { "tail" }),
            )
        })
        .unwrap();
    let a = initialize(&router, "/mcp").await;
    let sa = Some(a.as_str());
    let staff: &[(&str, &str)] = &[("x-tier", "staff")];

    // Catalog and registry members of every family, all unlisted.
    gate.unlist(&[
        "audit",
        "staff://handbook",
        "staff://people/{name}",
        "brief",
        "tail",
        "tail://notes",
        "tail_prompt",
    ]);
    assert_eq!(listed(&router, sa, staff, "tools").await, ["ping"]);
    assert!(listed(&router, sa, staff, "resources").await.is_empty());
    assert!(listed(&router, sa, staff, "templates").await.is_empty());
    assert!(listed(&router, sa, staff, "prompts").await.is_empty());

    // …and all of them still answer by name.
    assert_eq!(text_of(&call(&router, sa, staff, "audit").await), "audited");
    assert_eq!(text_of(&call(&router, sa, staff, "tail").await), "tail:x");
    for (uri, text) in [
        ("staff://handbook", "rules"),
        ("staff://people/ada", "ada"),
        ("tail://notes", "notes"),
    ] {
        let read = rpc(&router, sa, staff, "resources/read", json!({ "uri": uri })).await;
        assert_eq!(read["result"]["contents"][0]["text"], text, "{read}");
    }
    let sub = rpc(
        &router,
        sa,
        staff,
        "resources/subscribe",
        json!({ "uri": "staff://handbook" }),
    )
    .await;
    assert!(sub.get("error").is_none(), "{sub}");
    for (name, arguments) in [("brief", json!({})), ("tail_prompt", json!({ "text": "x" }))] {
        let got = rpc(
            &router,
            sa,
            staff,
            "prompts/get",
            json!({ "name": name, "arguments": arguments }),
        )
        .await;
        assert!(got["result"]["messages"].is_array(), "{got}");
    }

    // `listed` cannot bring back what `visible` hides: without the staff
    // header the group is gone, by name too.
    assert_eq!(call(&router, sa, &[], "audit").await["error"]["code"], -32601);
    gate.blocked.lock().unwrap().insert("tail".into());
    assert_eq!(call(&router, sa, staff, "tail").await["error"]["code"], -32601);
}

#[r2e_core::test]
async fn an_unlisted_private_member_still_shadows_its_registry_namesake() {
    let gate = Gate::default();
    let App {
        router,
        registry,
        sessions,
    } = gated(gate.clone()).await;
    let a = initialize(&router, "/mcp").await;
    let sa = Some(a.as_str());
    registry.add_tool(echoing(echo("dup"), "global")).unwrap();
    sessions
        .get(&a)
        .unwrap()
        .add_tool(echoing(echo("dup"), "private"))
        .unwrap();

    // A call resolves to the private member, so the list must not describe
    // the registry's under the same name.
    gate.unlist(&["dup"]);
    assert_eq!(listed(&router, sa, &[], "tools").await, ["ping"]);
    assert_eq!(text_of(&call(&router, sa, &[], "dup").await), "private:x");
}

#[r2e_core::test]
async fn unlisting_works_on_stateless_requests() {
    let gate = Gate::default();
    let App { router, .. } = boot!(
        AppBuilder::new().provide(gate.clone()),
        McpServer::new()
            .stateless(true)
            .json_response(true)
            .view_filter::<Gate>()
    );
    gate.unlist(&["ping"]);
    assert!(listed(&router, None, &[], "tools").await.is_empty());
    assert_eq!(text_of(&call(&router, None, &[], "ping").await), "pong");
}

#[cfg(feature = "loopback")]
#[r2e_core::test]
async fn unlisting_works_on_the_loopback() {
    use r2e_mcp::McpLoopback;
    use rmcp::model::CallToolRequestParams;

    let gate = Gate::default();
    let app = AppBuilder::new()
        .provide(gate.clone())
        .plugin(McpServer::new().view_filter::<Gate>())
        .build_state()
        .await
        .register_mcp_service::<Open>();
    let loopback = app.bean_context().try_get::<McpLoopback>().unwrap();
    let _router = app.build();
    gate.unlist(&["ping"]);

    let client = loopback.session("plain").connect().await.unwrap();
    assert!(client.list_all_tools().await.unwrap().is_empty());
    let result = client
        .call_tool(CallToolRequestParams::new("ping"))
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(false));
}

#[r2e_core::test]
#[should_panic(expected = "no `")]
async fn a_view_filter_without_its_bean_aborts_startup() {
    let _ = boot!(AppBuilder::new(), McpServer::new().view_filter::<Gate>());
}
